# Resolving rebase conflicts in DataZed

## Method

Upstream's version of a file is the new ground truth; DataZed's change is re-applied on top of it. During a rebase, `--ours` is the upstream side (plus DataZed commits already replayed) and `--theirs` is the DataZed commit being replayed.

1. `git status` names the conflicted files; `git show REBASE_HEAD` shows the DataZed commit being replayed, and its message says what the change is for.
2. For each file, take upstream's code and re-apply only what the table below lists for that file, adapted to upstream's new shape (a renamed function, a moved block, a changed signature).
3. Before `git rebase --continue`: no conflict markers remain, and the crate compiles, `cargo check -p <crate>` for each crate whose files you edited.

Done when every conflicted file is resolved this way. A resolution that drops one of the listed DataZed pieces, or one of upstream's changes, is not done; when the two cannot both survive, stop and describe the choice to the user.

## What DataZed changes in upstream files

Everything else DataZed adds lives in its own folders (`crates/r_tools`, `julia/DataZedVariables`, `.agents/skills/datazed-*`, `.github/workflows/datazed_release_rebase.yml`, `script/update-datazed`, `DATAZED.md`) and does not conflict.

| File | DataZed's piece to keep |
|---|---|
| `Cargo.lock` | Take upstream's file (`git checkout --ours Cargo.lock`), then `cargo metadata --format-version 1 >/dev/null` re-adds `r_tools`, `tree-sitter-r` and their dependencies |
| `Cargo.toml` | `"crates/r_tools"` in workspace members; `r_tools = { path = "crates/r_tools" }` and `tree-sitter-r` in workspace dependencies |
| `assets/keymaps/default-{macos,linux,windows}.json` | One block, the last in each file, with context `Editor && jupyter && (extension == r \|\| extension == py \|\| extension == jl)` binding `repl::Run`, `r_tools::SendToTerminal`, `ToggleVariables`, `OpenPlots` and `OpenConsole`. It must stay last: later blocks outrank earlier ones for the same key |
| `assets/settings/default.json` | `repl.run_by_statement` (after `max_lines`) and `repl.inline_plots` |
| `crates/settings_content/src/settings_content.rs`, `crates/repl/src/repl_settings.rs` | The same two settings as fields, with their defaults |
| `crates/repl/Cargo.toml` | `tree-sitter-r` dev-dependency |
| `crates/repl/src/repl.rs` | Re-export `KernelMessage` and `SessionEvent` beside `Session` |
| `crates/repl/src/repl_editor.rs` | `runnable_ranges` is `pub`; the `by_statement` check (languages from `repl.run_by_statement`, read with `ReplSettings::try_get`); `top_level_statement_at` with its syntax-error fallback and `with_trailing_semicolon`; a line-wise selection's end moved back to the last selected line; the R and Python tests |
| `crates/repl/src/session.rs` | The `followed` field; the `KernelMessage` struct and its `EventEmitter`; `route` emits `KernelMessage` only when followed; the inline PNG skip for followed sessions unless `inline_plots`; `send` is `pub` and returns an error when nothing was sent; the output block's `ScrollHandle` and scroll-wheel stop |
| `crates/repl/src/outputs.rs` | `ExecutionView::scroll_handle` |
| `crates/repl/src/kernels/mod.rs`, `native_kernel.rs` | `RunningKernel::connection_file()`, `None` by default, the connection path for local kernels; in the receive loop, the arm that logs and skips a `ParseError` whose `msg_type` is `comm_close`, before the arm that calls `kernel_errored` |
| `crates/terminal/src/terminal.rs` | `Terminal::foreground_process()` |
| `crates/release_channel/src/lib.rs` | Dev channel display name `DataZed`, app id `org.dwinkler.DataZed` |
| `crates/zed/Cargo.toml` | `r_tools` dependency; `bundle-dev` identifier `org.dwinkler.DataZed`, name `DataZed`, and `osx_url_schemes = ["datazed"]` |
| `crates/zed/src/zed/open_listener.rs` | `OpenRequest::parse` rewrites a `datazed://` URL to `zed://` before matching it; `test_parse_datazed_url` |
| `crates/zed/src/main.rs`, `crates/zed/src/zed.rs` | `r_tools::init(cx)` after `repl::init` in `main` and in the test setup of zed.rs; `datazed://` accepted in `parse_url_arg` (main.rs); `"r_tools"` in `test_action_namespaces`' list; no `install_cli::RegisterZedScheme` action in `register_actions` (and no imports only it used) |
| `crates/cli/src/main.rs` | `"datazed://"` in `URL_PREFIX`, so the `datazed` command passes such links to the app |
| `crates/zed_credentials_provider/src/zed_credentials_provider.rs` | The Dev channel uses the development credentials file only in debug builds (`cfg!(debug_assertions) &&`), so DataZed keeps secrets in the Keychain |
| `crates/install_cli/src/install_cli_binary.rs` | The CLI link is `/usr/local/bin/datazed` and its messages name `datazed`; DataZed does not register the `zed://` scheme (no `register_zed_scheme` call or import) |

If upstream renamed or removed an API one of these pieces uses (for example a `Session` method, a `ListItem` builder, a settings macro), adapt the piece to upstream's replacement in the same commit, and say so in the report to the user.
