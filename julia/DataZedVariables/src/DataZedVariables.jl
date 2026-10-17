"""
    DataZedVariables

Serves the `positron.variables` comm from an IJulia kernel, so DataZed's
variables panel, or any frontend that speaks Positron's variables protocol,
can list and inspect the session's global variables.

It depends only on IJulia and Julia's standard library, so it can live in
the global environment without constraining the packages of the project a
kernel runs in. Call [`start`](@ref) once per kernel, for example from
`~/.julia/config/startup_ijulia.jl`.
"""
module DataZedVariables

import IJulia

export start

include("describe.jl")
include("children.jl")
include("protocol.jl")
include("transport.jl")

"""The comm target frontends open; the same name Ark and Positron use."""
const TARGET = "positron.variables"

mutable struct State
    comm::Union{Nothing,IJulia.Comm}
    version::Int
    snapshot::Snapshot
end

const STATE = State(nothing, 0, Snapshot())
const STARTED = Ref(false)

"""
    start()

Send variable updates after every execution, including one that throws.
Opening the comm works without it; only the `update` events need IJulia's
hooks. Calling it again does nothing.
"""
function start()
    STARTED[] && return nothing
    missing = missing_internals()
    isempty(missing) || @warn "DataZedVariables: this IJulia ($(pkgversion(IJulia))) lacks $(join(missing, ", ")); \
        the variables panel will get no replies. DataZedVariables was checked against IJulia 1.34."
    IJulia.push_postexecute_hook(on_post_execute)
    IJulia.push_posterror_hook(on_post_execute)
    STARTED[] = true
    return nothing
end

# IJulia calls register_comm for every comm_open; dispatching on the target
# type is how a package claims a target.
function IJulia.register_comm(comm::IJulia.Comm{Symbol(TARGET)}, open_msg)
    guarded("opening the variables comm") do
        STATE.comm = comm
        comm.on_msg = request -> guarded(() -> on_request(comm, request), "answering a request")
        comm.on_close = _ -> (STATE.comm === comm && (STATE.comm = nothing))
        variables = global_variables(Main)
        STATE.snapshot = snapshot(Main, variables)
        STATE.version += 1
        params = Dict{String,Any}(
            "variables" => variables,
            "length" => length(variables),
            "version" => STATE.version,
        )
        send_on(comm, open_msg, event("refresh", params))
    end
end

function on_request(comm::IJulia.Comm, request::IJulia.Msg)
    data = request.content["data"]
    method = get(data, "method", nothing)
    params = something(get(data, "params", nothing), Dict{String,Any}())
    # Every request gets a reply, so a frontend never waits on a failed one.
    outcome = try
        Base.invokelatest(handle, method, params; root = Main, version = STATE.version)
    catch err
        RpcError(INTERNAL_ERROR, sprint(showerror, err))
    end
    send_on(comm, request, reply(get(data, "id", nothing), outcome))
    # Ark follows a clear with an update; no execution runs, so no hook sends it.
    method == "clear" && !(outcome isa RpcError) && send_updates(comm)
    return nothing
end

function on_post_execute()
    comm = STATE.comm
    comm === nothing || send_updates(comm)
    return nothing
end

# invokelatest: bindings added or deleted since this code was entered are only
# visible in the latest world.
function send_updates(comm::IJulia.Comm)
    guarded("sending variable updates") do
        variables = Base.invokelatest(global_variables, Main)
        assigned, removed, next = Base.invokelatest(changes, STATE.snapshot, Main, variables)
        isempty(assigned) && isempty(removed) && return nothing
        STATE.snapshot = next
        STATE.version += 1
        params = Dict{String,Any}(
            "assigned" => assigned,
            "removed" => removed,
            "version" => STATE.version,
        )
        send_on(comm, current_execution(), event("update", params))
    end
end

# Errors must never reach IJulia's event loop: log them and carry on.
function guarded(f, doing::AbstractString)
    try
        f()
    catch err
        @warn "DataZedVariables: failed $doing" exception = (err, catch_backtrace()) maxlog = 5
    end
    return nothing
end

end # module
