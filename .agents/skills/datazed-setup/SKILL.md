---
name: datazed-setup
description: Set up or troubleshoot DataZed on macOS: build, kernels (R, Python, Julia), console, settings. Use when someone asks to install or configure DataZed, or when one of its kernels, panels or the console misbehaves.
---

# DataZed setup

`DATAZED.md` at the repository root is the source of truth for every command, file and setting below; read the section a step names before running it, and quote its commands rather than retyping them from memory. This skill adds the order, the checks that tell a step is done, and the failures `DATAZED.md` does not spell out.

Everything here changes the user's machine. Before each install, download or file write, say what will change and get a yes: Homebrew and rustup installs, `Pkg.add`, `uv venv`, writing a kernelspec, `~/.julia/config/startup_ijulia.jl`, or Zed's `settings.json`. Read an existing file before changing it and merge into it; show the diff for `settings.json` and any kernelspec that already exists.

## 1. Inventory

Take the languages from the user's request; when it names none, ask which of R, Python and Julia they want, and set up only those. Ask whether DataZed still needs building. Then check what is present, one command each:

| Item | Check |
|---|---|
| Xcode tools | `xcode-select -p`; if it prints `/Library/Developer/CommandLineTools`, builds need `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` |
| cmake, rustup, uv | `command -v cmake rustup uv` |
| DataZed | `ls -d /Applications/DataZed.app` |
| Kernels | `ls ~/Library/Jupyter/kernels`, then each `kernel.json` |
| Zed extensions | `ls ~/Library/Application\ Support/Zed/extensions/installed` (`r` for R, `julia` for Julia; Python needs none) |
| R and Ark | `command -v R ark` |
| Positron | `ls -d /Applications/Positron.app` (Python variables) |
| Julia | `command -v julia`; `julia --version` (DataZedVariables needs 1.13 or newer); `julia -e 'using Pkg; Pkg.status()'` |

Done when every item the chosen languages need is marked present or missing, and the user has seen that table.

## 2. Build and install

Follow "Build (macOS, Apple silicon)". The build folder `target/` grows to tens of gigabytes; if the internal disk is short on space, offer to make `target` a symlink to a folder on another drive before the first build.

Done when `/Applications/DataZed.app/Contents/MacOS/zed` is newer than the build start and `codesign --verify --deep /Applications/DataZed.app` succeeds.

## 3. Kernels

Do each language the user chose. For each, follow its `DATAZED.md` section.

Zed extensions supply the language and grammar that statement running needs. The user installs them from DataZed's Extensions view; the agent cannot click in the app, so ask the user to do it and check the folder afterwards.

- **R** - "R setup": the R extension, Ark, `ark --install`. Done when the `r` extension folder exists, `~/Library/Jupyter/kernels/ark/kernel.json` exists, and its `argv[0]` is executable.
- **Python** - "Variables panel for Python" when Positron is installed; otherwise a plain `ipykernel` environment, which runs code and plots but shows no variables. Done when, with the interpreter and `PYTHONPATH` read back from the written `kernel.json`, `PYTHONPATH=<env.PYTHONPATH> <argv[0]> -Xfrozen_modules=off -c "import zmq, ipykernel, positron.positron_ipkernel"` succeeds (Positron route) or `<argv[0]> -c "import ipykernel"` succeeds (plain route).
- **Julia** - in order:
  1. The Julia extension and IJulia ("R setup"). Installing IJulia writes a `julia-<version>` kernelspec; if none appears, run `julia -e 'using Pkg; Pkg.build("IJulia")'`.
  2. DataZedVariables, the Julia package this repository ships in `julia/DataZedVariables` (https://github.com/dwinkler1/datazed/tree/data-tools/julia/DataZedVariables, "Variables panel for Julia"): install it into the global environment with that section's `Pkg.add` command. Do not pass the page URL to Pkg: Pkg reads `data-tools/julia/DataZedVariables` as the branch name and fails.
  3. The startup file from the same section, `~/.julia/config/startup_ijulia.jl`, which starts DataZedVariables in every IJulia kernel. If the file exists, add the section's block to it; if it already loads Positron.jl, ask before replacing that, since both packages register the same comm.

  `Pkg.add` has failed once with an `Unknown package` error and succeeded on a retry; retry once before investigating. Done when the `julia` extension folder exists, a `julia-*` kernelspec exists, `julia -e 'using DataZedVariables'` succeeds, and `startup_ijulia.jl` contains the section's block.
- **Console** - install uv ("R setup", step 4). Done when `command -v uvx` succeeds.

## 4. Settings

From "R setup" and "Settings": set `jupyter.kernel_selections` for each configured language to its kernelspec folder name (for example `ark`, `positron-python`, `julia-1.13`), and any `repl.*` setting the user asks for. DataZed shares Zed's settings file, `~/.config/zed/settings.json`; merge into it. It may be a symlink into a dotfiles repository: edit it in place so the link survives.

Done when the user has approved the diff and, after DataZed reloads the file, it shows no settings error notification.

## 5. Verify in DataZed

The agent cannot press keys in the app, so hand the user this checklist and record each answer. Ask them to run `repl: refresh kernelspecs` first, then, in a file of each configured language:

| Check | Expected |
|---|---|
| `cmd-enter` on a multi-line statement | the whole statement runs; output appears under it; the cursor moves to the next statement |
| `ctrl-alt-v` | the variables panel lists the session's objects (R with Ark, Python with Positron's kernel, Julia with DataZedVariables); other kernels show a "no variables view" message after about 10 idle seconds |
| a plot | it appears in the plots pane (`ctrl-alt-p` opens it); for Julia's Plots.jl, use `display(plot(...))` |
| `ctrl-alt-c` | a terminal tab opens a console on the same session; `ls()` (R), `dir()` (Python) or `varinfo()` (Julia) shows the editor's objects |
| `ctrl-cmd-enter` with that console tab active | the code runs in the console; in a plain shell tab it is refused with a toast |

When a check fails, read `~/Library/Logs/Zed/Zed.log` (shared with Zed) and the kernel's `kernel.json` before changing anything.

Done when every check for every configured language is marked passed or failed, and each failure has its log line or symptom recorded for the user.
