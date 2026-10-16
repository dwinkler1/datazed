# DataZed

DataZed is [Zed](https://github.com/zed-industries/zed) with tools for working in R through the [Ark](https://github.com/posit-dev/ark) kernel. Keys are for macOS; Linux and Windows use `ctrl-enter` for `cmd-enter` and `ctrl-alt-shift-enter` for `ctrl-cmd-enter`.

- `cmd-enter` runs the selection, or the whole top-level R statement at the cursor, and moves to the next statement.
- A variables panel lists the objects in the R session; data frames and lists expand.
- A plots pane shows R plots as they are drawn.
- `ctrl-cmd-enter` sends the same code to Zed's active terminal when it runs R, radian or the console below; it refuses a plain shell, where R code such as `x > y` would overwrite the file `y`.
- `ctrl-alt-c` opens a console in Zed's terminal attached to the same R session as the editor, so both see the same objects. The console prints only what runs in it; with its tab active, `ctrl-cmd-enter` sends code from the editor there.

The `r-tools` branch is a small stack of commits on top of the upstream Zed commit tagged `datazed/base`. Most of the code is in `crates/r_tools`; `crates/repl` has a few small hooks.

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
git clone --filter=blob:none --branch r-tools https://github.com/dwinkler1/datazed.git
cd datazed
script/bundle-mac -i
```

The first build takes roughly 15 to 30 minutes on an M2. `bundle-mac -i` installs `/Applications/DataZed.app`. It may then exit with an error at its DMG step; the install still worked if the output includes `Installed application bundle: /Applications/DataZed.app`. The app is signed ad hoc, so macOS may ask you to confirm on first launch (right-click, Open).

DataZed installs beside a regular Zed and shares its settings, extensions and logs. `zed: install cli` links the command `datazed`, not `zed`.

## R setup

1. In DataZed, open Extensions and install **R**.
2. Install Ark from its [releases page](https://github.com/posit-dev/ark/releases), put `ark` on your `PATH`, and run `ark --install`.
3. Run `repl: refresh kernelspecs`. To always use Ark for R, add to your settings:

```json
{ "jupyter": { "kernel_selections": { "r": "ark" } } }
```

4. For the console (`ctrl-alt-c`), install [uv](https://docs.astral.sh/uv/). The console runs `jupyter-console` through `uvx`, which downloads it on first use.

## Keys in `.R` files (macOS)

| Key | Action |
|---|---|
| `cmd-enter` | Run the selection or the statement at the cursor, then move to the next statement |
| `ctrl-alt-enter` | Run, keeping the cursor in place |
| `ctrl-cmd-enter` | Send the same code to the active terminal, if it runs R, radian or the console |
| `ctrl-alt-v` | Toggle the variables panel |
| `ctrl-alt-p` | Open the plots pane |
| `ctrl-alt-c` | Open a console attached to the R session; run some R code first so the session exists |

## Settings

| Setting | Default | Effect |
|---|---|---|
| `repl.inline_r_plots` | `false` | Show R plots under the code as well as in the plots pane |
| `repl.output_max_height_lines` | `0` (no limit) | Cap the height of output under the code; longer output scrolls inside its box |

## Updates

The branch is rebased onto new Zed releases and force-pushed. A daily GitHub workflow rebases it onto each new Zed release and runs the repl and r_tools checks on a `datazed-update/<tag>` branch; `r-tools` moves only after that release has been built and run on a Mac. To update a clone, discarding any local changes, and rebuild:

```sh
git fetch origin
git reset --hard origin/r-tools
script/bundle-mac -i
```

## Picking a subset

Each feature is its own commit, so you can take only some of them into your own Zed checkout. To see all changes, compare `datazed/base...r-tools` on GitHub; each commit there shows its own diff. Then fetch the branch and cherry-pick the commits you want, oldest first:

```sh
git remote add datazed https://github.com/dwinkler1/datazed.git
git fetch datazed r-tools tag datazed/base
git log --oneline --reverse datazed/base..datazed/r-tools   # the commits, oldest first
git cherry-pick <commit> <commit> ...
```

Commit hashes change with every rebase, so the table names commits by subject.

| Commit subject | Stands alone? | Needs |
|---|---|---|
| repl: run R by top-level tree-sitter statement or selection | yes | nothing |
| r_tools: add crate with send-to-terminal for R | no | the run-R-by-statement commit, whose keymap block it extends |
| repl: emit R kernel messages as a KernelMessage event | yes | nothing, but it has no visible effect alone |
| r_tools: add plot view fed by the R session | no | the r_tools crate and the KernelMessage event |
| r_tools: add variables panel backed by Ark's variables comm | no | the plot view commit, which adds the session tracker the panel uses |
| Rename the dev build to DataZed so it installs beside Zed | yes | nothing; skip it, or change the name and bundle id |
| repl: keep scroll inside a height-capped output block | yes | nothing |
| r_tools: open a Jupyter console attached to the R session | no | the variables panel |
| Add a workflow that rebases DataZed onto new Zed releases | yes | nothing; it runs only in dwinkler1/datazed, with the `DATAZED_PUSH_TOKEN` secret |
| Add DATAZED.md: what the fork adds, how to build it, how to pick a subset | yes | nothing; documentation only |

"Stands alone" means the commit applied cleanly by itself to `datazed/base` in a `git merge-tree` dry run. "Needs" also lists earlier commits whose code a commit uses, even where it would apply without them. On a `Cargo.lock` conflict, keep your branch's version (`git checkout --ours Cargo.lock` during a cherry-pick) and run `cargo check -p zed`, which adds the `r_tools` and `tree-sitter-r` entries back. The keymap files conflict if you have changed the same bindings.

Common subsets, with the commits to cherry-pick in this order. Each has passed `cargo check -p zed` and the tests of the crates it touches.

| You want | Commits, oldest first |
|---|---|
| Statement-based running, and sending code to a terminal | run R by statement; r_tools crate |
| The repl changes only, without the r_tools crate | run R by statement; KernelMessage event; scroll fix |
| The plots pane | run R by statement; r_tools crate; KernelMessage event; plot view |
| Everything | all commits in branch order |

## Limitations

- DataZed has been built and used on macOS only. The Linux and Windows bundle scripts still use Zed's "Zed Dev" names.
- Plots arrive at Ark's default size, not the size of the plots pane.
- When Ark is interrupted, it stops the running expression but still runs any later top-level expressions sent in the same submission.

Zed's licences apply; `crates/r_tools` is licensed GPL-3.0-or-later, like the Zed crates it builds on.
