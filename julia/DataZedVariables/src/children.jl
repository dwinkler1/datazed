# Children for `inspect` and resolving an inspect path back to a value. Each
# child's access key must lead back to it through `child`, so the two methods
# for a type are written together. The DataFrames extension adds tables.

const MAX_CHILDREN = 1000

"""
    children(value) -> Vector{Pair{String,Any}}

`access_key => child` for the first `MAX_CHILDREN` children of `value`.
"""
children(x::AbstractArray) =
    [string(i) => v for (i, v) in Iterators.take(pairs(IndexLinear(), x), MAX_CHILDREN)]
children(x::Tuple) = [string(i) => v for (i, v) in Iterators.take(enumerate(x), MAX_CHILDREN)]
children(x::NamedTuple) =
    [string(k) => v for (k, v) in Iterators.take(pairs(x), MAX_CHILDREN)]
children(x::AbstractDict) =
    [repr(k) => v for (k, v) in Iterators.take(x, MAX_CHILDREN)]
children(x) = struct_like(x) ?
    [string(f) => getfield(x, f) for f in Iterators.take(fieldnames(typeof(x)), MAX_CHILDREN)] :
    Pair{String,Any}[]

"""
    child(value, access_key)

The child of `value` that `children` listed under `access_key`; throws a
`KeyError` when there is none.
"""
child(x::Union{AbstractArray,Tuple}, key::AbstractString) = x[parse(Int, key)]
child(x::NamedTuple, key::AbstractString) = getfield(x, Symbol(key))
function child(x::AbstractDict, key::AbstractString)
    for (k, v) in x
        repr(k) == key && return v
    end
    throw(KeyError(key))
end
function child(x, key::AbstractString)
    struct_like(x) || throw(KeyError(key))
    field = Symbol(key)
    hasfield(typeof(x), field) || throw(KeyError(key))
    return getfield(x, field)
end

has_children(x::Union{AbstractArray,Tuple,NamedTuple,AbstractDict}) = !isempty(x)
has_children(x) = struct_like(x)

# Plain data structs. Scalars, strings, functions and types have internal
# fields that are not useful to browse.
function struct_like(x)
    x isa Union{Number,AbstractString,Symbol,Char,Function,Type,Module,Nothing,Missing,AbstractSet} && return false
    T = typeof(x)
    return isstructtype(T) && fieldcount(T) > 0
end

"""
    resolve(root::Module, path) -> value

Follow an inspect path: the first key is a global variable listed by
`global_variables`, the rest are child access keys. Throws a `KeyError` when
a step does not exist.
"""
function resolve(root::Module, path::AbstractVector)
    isempty(path) && throw(KeyError(path))
    name = Symbol(path[1])
    name in user_names(root) || throw(KeyError(path[1]))
    value = getfield(root, name)
    for key in path[2:end]
        value = child(value, string(key))
    end
    return value
end

child_variables(value) = [describe(display_name(value, key), v; access_key = key) for (key, v) in children(value)]

display_name(_, key::AbstractString) = key
