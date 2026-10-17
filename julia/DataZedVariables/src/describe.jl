# Turning a value into the variable record the protocol sends. Pure functions:
# no IJulia, no global state. The DataFrames extension adds methods for tables.

const MAX_VALUE_CHARS = 1000
const MAX_TYPE_CHARS = 100

"""Names IJulia or Julia put in `Main` that are not the user's variables."""
const HIDDEN = (:ans, :In, :Out, :include, :eval, :clear_history)

"""
    describe(name, value; access_key = name)

The protocol record for one variable: the four strings every frontend needs
(`access_key`, `display_name`, `display_value`, `display_type`) and the hints
`kind`, `length`, `has_children` and `is_truncated`.
"""
function describe(name::AbstractString, value; access_key::AbstractString = name)
    shown, truncated = display_value(value)
    # `length` and `isempty` run user code too; one failing value must not
    # fail the whole listing.
    len, browsable = try
        (value_length(value), has_children(value))
    catch
        (0, false)
    end
    return Dict{String,Any}(
        "access_key" => String(access_key),
        "display_name" => String(name),
        "display_value" => shown,
        "display_type" => display_type(value),
        "kind" => kind(value),
        "length" => len,
        "has_children" => browsable,
        "is_truncated" => truncated,
    )
end

"""
    display_value(value) -> (text, truncated)

A one-line, size-limited rendering. Showing a value runs user code, so a
failure becomes a placeholder instead of an error.
"""
function display_value(value)
    text = try
        sprint(value) do io, x
            show(IOContext(io, :limit => true, :compact => true, :displaysize => (10, 200)), x)
        end
    catch err
        "<error showing value: $(nameof(typeof(err)))>"
    end
    text = replace(text, '\n' => ' ')
    length(text) <= MAX_VALUE_CHARS && return (text, false)
    return (first(text, MAX_VALUE_CHARS), true)
end

# `show` renders a whole string before the text is cut, so cut it first.
function display_value(s::AbstractString)
    short = ncodeunits(s) > MAX_VALUE_CHARS ? first(s, MAX_VALUE_CHARS) : s
    text, truncated = invoke(display_value, Tuple{Any}, short)
    return (text, truncated || short != s)
end

function display_type(value)
    text = string(typeof(value))
    return length(text) <= MAX_TYPE_CHARS ? text : first(text, MAX_TYPE_CHARS) * "…"
end

kind(::Bool) = "boolean"
kind(::Number) = "number"
kind(::Union{AbstractString,Char,Symbol}) = "string"
kind(::Function) = "function"
kind(::AbstractDict) = "map"
kind(::Union{AbstractArray,Tuple,NamedTuple,AbstractSet}) = "collection"
kind(::Union{Nothing,Missing}) = "empty"
kind(::Type) = "class"
kind(_) = "other"

value_length(x::Union{AbstractArray,AbstractDict,Tuple,NamedTuple,AbstractSet,AbstractString}) = length(x)
value_length(_) = 0

"""
    user_names(root::Module) -> Vector{Symbol}

The user's variables in `root`, sorted: names defined in `root` itself (not
imported, not reached through `using`), without modules, IJulia's history
names and generated `#` names.
"""
user_names(root::Module) =
    sort!(filter(name -> listed(root, name), names(root; all = true, imported = false)); by = string)

"""Records for every variable in `user_names(root)`."""
global_variables(root::Module) =
    [describe(string(name), getfield(root, name)) for name in user_names(root)]

function listed(root::Module, name::Symbol)
    name in HIDDEN && return false
    startswith(string(name), '#') && return false
    isdefined(root, name) || return false
    return !(getfield(root, name) isa Module)
end
