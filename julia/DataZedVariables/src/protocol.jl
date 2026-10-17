# The positron.variables protocol as data in, data out: request handling,
# snapshots and the update calculation. No IJulia calls happen here.

# JSON-RPC error codes this package returns.
const METHOD_NOT_FOUND = -32601
const INVALID_PARAMS = -32602
const INTERNAL_ERROR = -32603
const NOT_SUPPORTED = -32000

struct RpcError
    code::Int
    message::String
end

"""
    handle(method, params; root, version) -> Dict or RpcError

The result for one request. `list` returns every variable, `inspect` the
children at `params["path"]`, and `clear` removes every variable with
`Base.delete_binding`, an internal function of Julia 1.13. `delete` is refused.
"""
function handle(method, params::AbstractDict; root::Module, version::Integer)
    if method == "list"
        variables = global_variables(root)
        return Dict{String,Any}("variables" => variables, "length" => length(variables), "version" => version)
    elseif method == "inspect"
        path = get(params, "path", nothing)
        path isa AbstractVector || return RpcError(INVALID_PARAMS, "inspect needs a `path` list")
        value = try
            resolve(root, path)
        catch
            return RpcError(INVALID_PARAMS, "nothing at path $(join(path, " → "))")
        end
        kids = child_variables(value)
        return Dict{String,Any}("children" => kids, "length" => length(kids))
    elseif method == "clear"
        foreach(name -> Base.delete_binding(root, name), user_names(root))
        return Dict{String,Any}()
    elseif method == "delete"
        return RpcError(NOT_SUPPORTED, "delete is not supported; Remove all objects uses clear")
    else
        return RpcError(METHOD_NOT_FOUND, "unknown method: $(something(method, "(none)"))")
    end
end

"""A JSON-RPC response for `outcome`, answering the request with `id`."""
reply(id, result::AbstractDict) = Dict{String,Any}("jsonrpc" => "2.0", "id" => id, "result" => result)
reply(id, err::RpcError) = Dict{String,Any}(
    "jsonrpc" => "2.0",
    "id" => id,
    "error" => Dict{String,Any}("code" => err.code, "message" => err.message),
)

"""A JSON-RPC notification, the shape of `refresh` and `update` events."""
event(method::AbstractString, params::AbstractDict) =
    Dict{String,Any}("jsonrpc" => "2.0", "method" => method, "params" => params)

"""
What was last sent per variable: access key => (display_value, display_type,
fingerprint). The fingerprint catches in-place changes the display text hides.
"""
const Snapshot = Dict{String,Tuple{String,String,UInt}}

snapshot(root::Module, variables) = Snapshot(
    v["access_key"] =>
        (v["display_value"], v["display_type"], fingerprint(getfield(root, Symbol(v["access_key"]))))
    for v in variables
)

const FULL_HASH_LIMIT = 1_000_000

"""
    fingerprint(value) -> UInt

A content hash. `Base.hash` samples the elements of large arrays, so arrays of
up to `FULL_HASH_LIMIT` elements hash every element; larger arrays and mutable
structs (hashed by identity) can still change unseen.
"""
function fingerprint(value)
    try
        if value isa AbstractArray && length(value) <= FULL_HASH_LIMIT
            return foldl((h, x) -> hash(x, h), value; init = hash(size(value)))
        end
        return hash(value)
    catch
        return UInt(0)
    end
end

"""
    changes(previous::Snapshot, root::Module, variables) -> (assigned, removed, snapshot)

Variables that are new or whose value, type text or fingerprint changed, access
keys that disappeared, and the snapshot to compare against next time.
"""
function changes(previous::Snapshot, root::Module, variables)
    current = snapshot(root, variables)
    assigned = [v for v in variables if get(previous, v["access_key"], nothing) != current[v["access_key"]]]
    removed = sort!([key for key in keys(previous) if !haskey(current, key)])
    return assigned, removed, current
end
