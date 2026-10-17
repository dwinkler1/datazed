# Sending comm messages with the right parent. This is the only file that
# uses IJulia internals (checked against IJulia 1.34): msg_comm, send_ipython,
# _default_kernel and its publish socket and execute_msg.
#
# IJulia's public send_comm always names the last executed cell as the
# parent. A reply must name the request it answers, because frontends such as
# DataZed match replies to requests by their parent message.

"""Send `data` as a comm_msg on `comm`, with `parent` as the parent message."""
function send_on(comm::IJulia.Comm, parent::IJulia.Msg, data::AbstractDict)
    kernel = IJulia._default_kernel
    msg = IJulia.msg_comm(comm, parent, "comm_msg", data)
    IJulia.send_ipython(kernel.publish[], kernel, msg)
    return nothing
end

"""The IJulia internals this file uses that the installed IJulia lacks."""
function missing_internals()
    missing = [name for name in (:_default_kernel, :msg_comm, :send_ipython) if !isdefined(IJulia, name)]
    kernel_fields = isdefined(IJulia, :Kernel) ? fieldnames(IJulia.Kernel) : ()
    append!(missing, [Symbol("Kernel.", f) for f in (:publish, :execute_msg) if !(f in kernel_fields)])
    return missing
end

"""The execute request currently running, the parent for update events."""
current_execution() = IJulia._default_kernel.execute_msg
