using DataZedVariables
using DataZedVariables: describe, display_value, global_variables, children, child, resolve,
    handle, reply, event, changes, snapshot, RpcError, Snapshot, MAX_VALUE_CHARS, MAX_CHILDREN
using DataFrames: DataFrame
using Test

struct Point
    x::Int
    y::Float64
end

struct Unshowable end
Base.show(::IO, ::Unshowable) = error("boom")

struct Unsized <: AbstractVector{Int} end
Base.size(::Unsized) = error("no size")
Base.show(io::IO, ::Unsized) = print(io, "Unsized()")

# A stand-in for Main with a few kinds of variables.
module Session
    using DataFrames: DataFrame
    import ..Point
    n = 42
    name = "text"
    v = [10, 20, 30]
    d = Dict("a" => [1, 2, 3], :b => "two")
    nt = (alpha = 1, beta = "b")
    p = Point(1, 2.5)
    df = DataFrame(a = 1:3, b = ["x", "y", "z"])
    s = Set([1, 2])
    t = (1, "two")
    f(x) = x + 1
end

module Clearable
    a = 1
    b = [1, 2]
end

module Changing
    a = 1
    big = zeros(10_000)
end

key_names(records) = [r["access_key"] for r in records]

@testset "DataZedVariables" begin
    @testset "describe" begin
        record = describe("n", 42)
        @test record["access_key"] == "n"
        @test record["display_value"] == "42"
        @test record["display_type"] == "Int64"
        @test record["kind"] == "number"
        @test record["has_children"] == false
        @test describe("v", [1, 2])["kind"] == "collection"
        @test describe("v", [1, 2])["length"] == 2
        @test describe("s", "abc")["kind"] == "string"
        @test describe("nothing", nothing)["kind"] == "empty"
        long, truncated = display_value(collect(1:10_000))
        @test length(long) <= MAX_VALUE_CHARS
        @test !occursin('\n', long)
        @test startswith(display_value(Unshowable())[1], "<error showing value")
        unsized = describe("u", Unsized())
        @test (unsized["length"], unsized["has_children"]) == (0, false)
        text, truncated = display_value("a"^5_000_000)
        @test truncated && length(text) <= MAX_VALUE_CHARS
        @test display_value("short") == ("\"short\"", false)
    end

    @testset "global variables" begin
        names = key_names(global_variables(Session))
        @test names == sort(names)
        @test all(in(names), ["n", "name", "v", "d", "nt", "p", "df", "s", "f"])
        @test !any(in(names), ["DataFrame", "Point", "eval", "include"])
    end

    @testset "children and resolve" begin
        @test first.(children([10, 20])) == ["1", "2"]
        @test child([10, 20], "2") == 20
        @test length(children(collect(1:5000))) == MAX_CHILDREN
        @test Dict(children(Session.d))["\"a\""] == [1, 2, 3]
        @test child(Session.d, ":b") == "two"
        @test first.(children(Session.nt)) == ["alpha", "beta"]
        @test first.(children(Point(1, 2.0))) == ["x", "y"]
        @test isempty(children(Set([1])))
        @test children((10, "a")) == ["1" => 10, "2" => "a"]
        @test child((10, "a"), "2") == "a"
        @test describe("t", (10, "a"))["has_children"]
        @test resolve(Session, ["d", "\"a\"", "2"]) == 2
        @test resolve(Session, ["p", "y"]) == 2.5
        @test_throws KeyError resolve(Session, ["no_such_variable"])
        @test_throws KeyError resolve(Session, ["missing"])  # Base binding, not a variable
        @test_throws KeyError resolve(Session, ["p", "z"])
    end

    @testset "DataFrames extension" begin
        record = describe("df", Session.df)
        @test record["kind"] == "table"
        @test record["display_value"] == "[3 rows × 2 columns] DataFrame"
        @test record["length"] == 3
        @test first.(children(Session.df)) == ["a", "b"]
        @test resolve(Session, ["df", "b"]) == ["x", "y", "z"]
        wide = DataFrame([Symbol("c", i) => [i] for i in 1:(MAX_CHILDREN + 5)])
        @test length(children(wide)) == MAX_CHILDREN
    end

    @testset "requests" begin
        listing = handle("list", Dict(); root = Session, version = 7)
        @test listing["version"] == 7
        @test listing["length"] == length(listing["variables"])
        inspected = handle("inspect", Dict("path" => ["nt"]); root = Session, version = 7)
        @test key_names(inspected["children"]) == ["alpha", "beta"]
        @test handle("inspect", Dict("path" => ["nt", "gamma"]); root = Session, version = 7) isa RpcError
        @test handle("inspect", Dict(); root = Session, version = 7) isa RpcError
        @test key_names(handle("inspect", Dict("path" => ["t"]); root = Session, version = 7)["children"]) == ["1", "2"]
        @test handle("delete", Dict(); root = Session, version = 7).code == DataZedVariables.NOT_SUPPORTED
        @test handle("bogus", Dict(); root = Session, version = 7).code == DataZedVariables.METHOD_NOT_FOUND
        @test isempty(DataZedVariables.missing_internals())
        @test reply("id-1", listing)["result"] === listing
        @test reply("id-1", RpcError(1, "no"))["error"]["message"] == "no"
        @test event("refresh", Dict("x" => 1))["method"] == "refresh"
    end

    @testset "clear" begin
        @test handle("clear", Dict(); root = Clearable, version = 7) == Dict{String,Any}()
        @test isempty(Base.invokelatest(global_variables, Clearable))
    end

    @testset "updates" begin
        variables() = Base.invokelatest(global_variables, Changing)
        previous = snapshot(Changing, variables())
        Core.eval(Changing, :(a = 2; c = 3))
        assigned, removed, previous = changes(previous, Changing, variables())
        @test key_names(assigned) == ["a", "c"]
        @test isempty(removed)

        shown_before = describe("big", Changing.big)["display_value"]
        Changing.big[5_000] = 1.0
        @test describe("big", Changing.big)["display_value"] == shown_before
        assigned, _, previous = changes(previous, Changing, variables())
        @test key_names(assigned) == ["big"]

        assigned, removed, _ = changes(previous, Changing, variables())
        @test isempty(assigned) && isempty(removed)
        @test changes(Snapshot(), Changing, Dict{String,Any}[])[1] == []
    end
end
