# DataZed

DataZed is [Zed](https://github.com/zed-industries/zed) with tools for running code through Jupyter kernels, built for R with the [Ark](https://github.com/posit-dev/ark) kernel. Statement running, the plots pane, the console and sending code to a terminal also work with Python and Julia kernels. Keys are for macOS; Linux and Windows use `ctrl-enter` for `cmd-enter` and `ctrl-alt-shift-enter` for `ctrl-cmd-enter`.

- `cmd-enter` runs the selection, or the whole top-level statement at the cursor, and moves to the next statement (for the languages in `repl.run_by_statement`: R, Python and Julia by default).
- A variables panel lists the objects in the session; data frames and lists expand. It needs Ark, Positron's Python kernel or DataZedVariables in IJulia (set up below); with other kernels it says so.
- A plots pane shows plots as they are drawn.
- `ctrl-cmd-enter` sends the same code to Zed's active terminal when it runs R, radian, Python, IPython, Julia or the console below; it refuses a plain shell, where code such as `x > y` would overwrite the file `y`.
- `ctrl-alt-c` opens a console in Zed's terminal attached to the same kernel session as the editor, so both see the same objects. The console prints only what runs in it; with its tab active, `ctrl-cmd-enter` sends code from the editor there.

An agent can walk you through all of the setup below with the skill in `.agents/skills/datazed-setup`.

The `data-tools` branch is a small stack of commits on top of the upstream Zed commit tagged `datazed/base`. Most of the code is in `crates/r_tools`; `crates/repl` has a few small hooks.

## Build (macOS, Apple silicon)

One-time setup:

```sh
# Install Xcode from the App Store and launch it once, then:
xcode-select --install
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
brew install cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # rustup fetches the Rust version pinned in rust-toolchain.toml
```

Clone and build:

```sh
git clone --filter=blob:none --branch data-tools https://github.com/dwinkler1/datazed.git
cd datazed
script/bundle-mac -i
```

The first build takes roughly 15 to 30 minutes on an M2. `bundle-mac -i` installs `/Applications/DataZed.app`. It may then exit with an error at its DMG step; the install still worked if the output includes `Installed application bundle: /Applications/DataZed.app`. The app is signed ad hoc, so macOS may ask you to confirm on first launch (right-click, Open).

DataZed installs beside a regular Zed and shares its settings, extensions and logs. `cli: install cli binary` (DataZed > Install CLI) links the command `datazed`, not `zed`. Links starting with `datazed://` open in DataZed and do what the same `zed://` link does in Zed, for example `datazed://settings`, including from the `datazed` command; `zed://` links keep opening Zed. Links that DataZed itself copies or shares, such as a settings link, still start with `zed://` and open in Zed.

## R setup

1. In DataZed, open Extensions and install **R**.
2. Install Ark from its [releases page](https://github.com/posit-dev/ark/releases), put `ark` on your `PATH`, and run `ark --install`.
3. Run `repl: refresh kernelspecs`. To always use Ark for R, add to your settings:

```json
{ "jupyter": { "kernel_selections": { "r": "ark" } } }
```

4. For the console (`ctrl-alt-c`), install [uv](https://docs.astral.sh/uv/). The console runs `jupyter-console` through `uvx`, which downloads it on first use.

For Python, install `ipykernel` in the environment you run; for Julia, install the Julia extension and IJulia (`julia -e 'using Pkg; Pkg.add("IJulia")'`), which also writes a `julia-<version>` kernelspec. Then run `repl: refresh kernelspecs`.

### Variables panel for Julia

IJulia has no variables comm of its own. [`julia/DataZedVariables`](https://github.com/dwinkler1/datazed/tree/data-tools/julia/DataZedVariables) in this repository is a small Julia package that adds the `positron.variables` comm to IJulia. It depends only on IJulia and Julia's standard library, so it can sit in your global environment without constraining the packages of the project a kernel runs in. It needs Julia 1.13 or newer. Install it once, then start it in every IJulia kernel through `~/.julia/config/startup_ijulia.jl`. Install with the command below, not the folder's GitHub page URL: Pkg reads everything after `/tree/` as the branch name.

```sh
julia -e 'using Pkg; Pkg.add(url="https://github.com/dwinkler1/datazed", subdir="julia/DataZedVariables", rev="data-tools")'
```

```julia
# ~/.julia/config/startup_ijulia.jl
try
    using DataZedVariables
    DataZedVariables.start()
catch err
    @warn "startup_ijulia.jl: variables comm not set up" exception = err
end
```

Data frames show their size and columns when the project has DataFrames. "Remove all objects" deletes every variable the panel lists, through Julia's internal `Base.delete_binding`. Plots stay ordinary image output. With Plots.jl, a `plot(...)` on a cell's last line does not reach the plots pane or the inline output yet: Plots.jl returns it as the cell's result, which DataZed shows as text only. Use `display(plot(...))` instead.

[Positron.jl](https://github.com/TidierOrg/positron-julia) from TidierOrg's positron-julia provides the same comm but brings DataFrames, JSON3 and their dependencies into the kernel, which breaks in projects that need other versions of them (for example Parsers 3). Load one of the two packages, not both: they register the same comm.

### Variables panel for Python

The standard IPython kernel has no variables comm. Positron's Python kernel does, and it ships inside [Positron](https://positron.posit.co): Posit does not publish it as a separate package. With Positron installed, a Jupyter kernelspec can start that kernel with your own Python.

1. Make a Python environment with your packages. Use Python 3.13 or older: the kernel's bundled libraries fail on 3.15 (3.14 is untested). It needs no `ipykernel` or `ipython`, since Positron bundles them.

   ```sh
   uv venv --python 3.13 ~/.venvs/positron-py
   uv pip install --python ~/.venvs/positron-py numpy pandas matplotlib
   ```

2. Create `~/Library/Jupyter/kernels/positron-python/kernel.json`, with `<you>` replaced by your user name:

   ```json
   {
     "argv": [
       "/Users/<you>/.venvs/positron-py/bin/python",
       "-Xfrozen_modules=off",
       "/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/posit/positron_language_server.py",
       "-f", "{connection_file}",
       "--session-mode=notebook"
     ],
     "env": {
       "PYTHONPATH": "/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/lib/ipykernel/py3:/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/lib/ipykernel/arm64/cp3:/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/lib/ipykernel/universal2/cp312-abi3:/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/lib/python:/Applications/Positron.app/Contents/Resources/app/extensions/positron-python/python_files/posit"
     },
     "display_name": "Positron Python",
     "language": "python"
   }
   ```

   `PYTHONPATH` adds Positron's bundled `ipykernel`, IPython and compiled `pyzmq`; the `cp312-abi3` build of `pyzmq` serves Python 3.12 and later, and older Pythons need the matching `universal2/cp3XX` folder instead. `--session-mode=notebook` keeps plots as ordinary `image/png` output for the plots pane. `-Xfrozen_modules=off` silences a debugger warning.

3. Select it for Python in your settings and run `repl: refresh kernelspecs`:

   ```json
   { "jupyter": { "kernel_selections": { "python": "positron-python" } } }
   ```

The kernel runs from inside `Positron.app`, so a Positron update can move these files; if the kernel stops starting, check the paths above. Its code is © Posit Software and licensed under the Elastic License 2.0; it stays in your Positron installation and DataZed neither contains nor redistributes it. Tested with Python 3.13: the variables panel's list, inspect and updates, and PNG output. Not yet tested: matplotlib plots, and the kernel inside DataZed.

## Keys in `.R`, `.py` and `.jl` files (macOS)

| Key | Action |
|---|---|
| `cmd-enter` | Run the selection or the statement at the cursor, then move to the next statement |
| `ctrl-alt-enter` | Run, keeping the cursor in place |
| `ctrl-cmd-enter` | Send the same code to the active terminal, if it runs R, radian, Python, IPython, Julia or the console |
| `ctrl-alt-v` | Toggle the variables panel |
| `ctrl-alt-p` | Open the plots pane |
| `ctrl-alt-c` | Open a console attached to the kernel session; run some code first so the session exists |

## Settings

| Setting | Default | Effect |
|---|---|---|
| `repl.run_by_statement` | `["R", "Python", "Julia"]` | Languages whose code runs by top-level statement instead of by block of non-blank lines |
| `repl.inline_plots` | `false` | Show plots under the code as well as in the plots pane |
| `repl.output_max_height_lines` | `0` (no limit) | Cap the height of output under the code; longer output scrolls inside its box |

## Updates

The branch is rebased onto new Zed releases and force-pushed. A daily GitHub workflow rebases it onto each new Zed release and runs the repl and r_tools checks on a `datazed-update/<tag>` branch; `data-tools` moves only after that release has been built and run on a Mac. On a Mac, `script/update-datazed` does that rebase, the checks and the rebuild in one run, and `--push` publishes the result; the agent skill in `.agents/skills/datazed-update` walks an agent through the same steps, including resolving merge conflicts. To update a clone, discarding any local changes, and rebuild:

```sh
git fetch origin
git reset --hard origin/data-tools
script/bundle-mac -i
```

## Picking a subset

Each feature is its own commit, so you can take only some of them into your own Zed checkout. To see all changes, compare `datazed/base...data-tools` on GitHub; each commit there shows its own diff. Then fetch the branch and cherry-pick the commits you want, oldest first:

```sh
git remote add datazed https://github.com/dwinkler1/datazed.git
git fetch --force datazed data-tools tag datazed/base
git log --oneline --reverse datazed/base..datazed/data-tools   # the commits, oldest first
git cherry-pick <commit> <commit> ...
```

Commit hashes change with every rebase, so the table names commits by subject.

| Commit subject | Stands alone? | Needs |
|---|---|---|
| repl: skip a comm_close that does not parse instead of failing the kernel | yes | nothing; without it, Ark closing a comm (for example `View(df)` then `rm(df)`) ends the R session |
| repl: run code by top-level tree-sitter statement or selection | yes | nothing |
| r_tools: add crate with send-to-terminal | no | the run-by-statement commit, whose keymap block it extends |
| repl: emit KernelMessage events for followed sessions | yes | nothing, but it has no visible effect alone |
| r_tools: add plot view fed by the REPL session | no | the r_tools crate and the KernelMessage event |
| r_tools: add variables panel backed by the positron.variables comm | no | the plot view commit, which adds the session tracker the panel uses |
| Rename the dev build to DataZed so it installs beside Zed | yes | nothing; skip it, or change the name and bundle id |
| repl: keep scroll inside a height-capped output block | yes | nothing |
| r_tools: open a Jupyter console attached to the REPL session | no | the variables panel |
| julia: add DataZedVariables, which serves the variables panel from IJulia | yes | nothing; a separate Julia package that the variables panel talks to |
| Add a workflow and a script that rebase DataZed onto new Zed releases | yes | nothing; the workflow runs only in dwinkler1/datazed, with the `DATAZED_PUSH_TOKEN` secret |
| Add DATAZED.md and DataZed's agent skills | yes | nothing; documentation and agent skills |

"Stands alone" means the commit applied cleanly by itself to `datazed/base` in a `git merge-tree` dry run. "Needs" also lists earlier commits whose code a commit uses, even where it would apply without them. On a `Cargo.lock` conflict, keep your branch's version (`git checkout --ours Cargo.lock` during a cherry-pick) and run `cargo check -p zed`, which adds the `r_tools` and `tree-sitter-r` entries back. The keymap files conflict if you have changed the same bindings.

Common subsets, with the commits to cherry-pick in this order. Each has passed `cargo check -p zed` and the tests of the crates it touches.

| You want | Commits, oldest first |
|---|---|
| Statement-based running, and sending code to a terminal | run by statement; r_tools crate |
| The repl changes only, without the r_tools crate | run by statement; KernelMessage event; scroll fix |
| The plots pane | run by statement; r_tools crate; KernelMessage event; plot view |
| Everything | all commits in branch order |

## Limitations

- DataZed has been built and used on macOS only. The Linux and Windows bundle scripts still use Zed's "Zed Dev" names; on Linux the window's app id is DataZed's, so it no longer matches that desktop entry (icon and window grouping).
- Plots arrive at Ark's default size, not the size of the plots pane.
- When Ark is interrupted, it stops the running expression but still runs any later top-level expressions sent in the same submission.
- Remote development over SSH, WSL or Docker does not work. DataZed is built on Zed's Dev channel, which expects a locally built remote server instead of downloading one.
- On Linux and Windows, ctrl-enter runs code in R, Python and Julia files, so it no longer opens the Inline Assistant there.
- DataZed's keys for R, Python and Julia files, including send to terminal, apply only while Jupyter is enabled (`jupyter.enabled`, on by default).
- "Check for Updates" does nothing, as in any Dev build: update with `script/update-datazed` or the datazed-update skill. Crash reports are off, and Zed's performance profiler setting defaults to on, as on Zed's Dev channel.
- A `datazed://` link from a web page can do anything a `zed://` link can in Zed, such as open a file, start an SSH connection or prefill an agent prompt, after the browser asks to open DataZed. DataZed does not update itself, so fixes Zed makes to those handlers arrive when you rebuild.

## Acknowledgements and licences

DataZed is a fork of [Zed](https://github.com/zed-industries/zed) by Zed Industries. Zed's licences apply unchanged: `LICENSE-GPL` and `LICENSE-APACHE` at the repository root, with each crate's licence declared in its `Cargo.toml`. DataZed's own code is GPL-3.0-or-later: the new `crates/r_tools`, and its changes to the `repl`, `terminal`, `settings_content`, `release_channel`, `install_cli`, `cli`, `zed_credentials_provider` and `zed` crates, which are all GPL-3.0-or-later in Zed. The Julia package in `julia/DataZedVariables` is separate from the app and MIT-licensed; see its `LICENSE`. `script/bundle-mac` runs `script/generate-licenses`, which bundles the licences of every dependency shipped in the app.

DataZed builds on these projects without including their code:

| Project | Licence | How DataZed uses it |
|---|---|---|
| [Ark](https://github.com/posit-dev/ark) by Posit | MIT | The R kernel DataZed is built for. The variables panel speaks Ark's `positron.variables` comm. |
| [Positron](https://github.com/posit-dev/positron) by Posit | Elastic License 2.0 | Defines the `positron.variables` comm protocol that the variables panel implements. Its Python kernel, started from an installed Positron, gives Python sessions the variables panel. DataZed contains no Positron code. |
| [positron-julia](https://github.com/TidierOrg/positron-julia) (Positron.jl) by TidierOrg, © Posit Software | Elastic License 2.0 | An alternative to DataZedVariables: provides the `positron.variables` comm in IJulia. You install it yourself under its licence; DataZed neither contains nor redistributes it. |
| [IJulia](https://github.com/JuliaLang/IJulia.jl) | MIT | The Julia kernel. |
| [jupyter-console](https://github.com/jupyter/jupyter_console) | BSD-3-Clause | The console that `ctrl-alt-c` runs through `uvx`; it is downloaded at first use, not bundled. |
| [tree-sitter-r](https://github.com/r-lib/tree-sitter-r) | MIT | Test-only dependency of `crates/repl`; not shipped in the app. |
| [zed-r](https://github.com/ocsmit/zed-r) and [zed-julia](https://github.com/JuliaEditorSupport/zed-julia) | MIT | The Zed extensions that supply the R and Julia grammars statement picking reads; installed from Zed's extension registry. |
