# Loaded only when the kernel's environment has DataFrames: tables show their
# size, and their columns are the children.
module DataZedVariablesDataFramesExt

using DataFrames: DataFrames, AbstractDataFrame, ncol, nrow
import DataZedVariables: MAX_CHILDREN, child, children, display_type, display_value, has_children, kind, value_length

display_value(df::AbstractDataFrame) = ("[$(nrow(df)) rows × $(ncol(df)) columns] $(nameof(typeof(df)))", false)
display_type(df::AbstractDataFrame) = "$(nameof(typeof(df))) [$(nrow(df))×$(ncol(df))]"
kind(::AbstractDataFrame) = "table"
value_length(df::AbstractDataFrame) = nrow(df)
has_children(df::AbstractDataFrame) = ncol(df) > 0

children(df::AbstractDataFrame) =
    [name => df[!, name] for name in Iterators.take(names(df), MAX_CHILDREN)]

function child(df::AbstractDataFrame, key::AbstractString)
    key in names(df) || throw(KeyError(key))
    return df[!, key]
end

end # module
