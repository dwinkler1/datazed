---
name: datazed-update
description: DataZed update - rebase the data-tools branch onto a newer Zed release, resolve its merge conflicts, and rebuild. Use when someone asks to update DataZed to a new Zed version, or when a rebase of data-tools, `script/update-datazed` or the DataZed release workflow stops on a conflict or a failed check.
---

# DataZed update

DataZed is a stack of commits on branch `data-tools`, on top of the upstream Zed commit tagged `datazed/base`. Updating means replaying that stack onto a newer Zed release tag. The stack stays one commit per feature, with the subjects `DATAZED.md`'s subset table names; a fix belongs in the commit it fixes, never in a new commit on top.

`script/update-datazed` does the mechanical part: backup tag, rebase, `Cargo.lock` conflicts, moving `datazed/base`, checks, build, launch check and push (`--help` lists its options). This skill runs it and does what it leaves to a person: choosing the target, resolving other conflicts, folding fixes into their commits, and the subset table.

Ask before pushing or deleting anything on a remote. Everything before the push stays local and can be undone from the backup tag the script makes, `datazed/backup-<time>`.

## 1. Pick the target

Find the remotes with `git remote -v`: the **upstream** points at `zed-industries/zed`, the **fork** at `dwinkler1/datazed`. Fetch upstream's tags (`git fetch --tags --force <upstream>`, or Zed's GitHub URL when no remote points there; `--force` because upstream moves its `nightly` tag) and take the newest stable tag, `git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1`. The tag is newer when `git merge-base --is-ancestor datazed/base <tag>` succeeds, or when its `version` in `crates/zed/Cargo.toml` is higher than the one at `datazed/base`. `datazed/base` may sit on `main` ahead of the newest release, and then there is nothing to do.

The fork's branches show what the daily release workflow found for that tag: `datazed-update/<tag>` means the rebase, clippy, the repl and r_tools tests and `cargo check -p zed` passed on Linux; `datazed-update/<tag>-failed` means it stopped on a conflict or a failed check, and its run summary names which.

Done when the target tag is named and newer than the base, or the user has been told the branch is already current.

## 2. Rebase

From a clean `data-tools` checkout, save the stack's subjects (`git log --format=%s datazed/base..data-tools`), then run:

```sh
script/update-datazed --no-build <tag>
```

When it stops with "Conflicts to resolve by hand", read [`conflicts.md`](conflicts.md) and resolve as it says; then `git add` the files and `git rebase --continue`, repeating until the rebase finishes. The script itself is missing from the checkout until then. When a conflict resolves to no change, `git rebase --continue` reports nothing to commit and `git rebase --skip` drops that commit; git drops a commit that upstream already contains without stopping. Then rerun the same command: it moves `datazed/base` and runs the checks.

Done when the script has printed "Checking repl and r_tools", no rebase is in progress, `git diff --no-ext-diff datazed/base..data-tools | grep -E '^\+(<<<<<<<|>>>>>>>)'` finds no conflict marker in any line DataZed adds (`--no-ext-diff` keeps an external diff tool from hiding them), and the subjects match the saved list in order, apart from the script's `Cargo.lock: re-resolve…` commit. Name each dropped subject for step 4 and the report.

## 3. Check

The script runs clippy with warnings denied on `repl`, `r_tools` and `terminal`, and the `repl` and `r_tools` tests. Then compile DataZed's changes in the other crates, as the release workflow does; `DEVELOPER_DIR` is needed when `xcode-select -p` points at the command-line tools, which lack the `metal` shader compiler:

```sh
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer cargo check -p zed
```

Fold every fix into the commit it fixes: `git commit --fixup=<sha>`, then `GIT_SEQUENCE_EDITOR=true git rebase -i --autosquash datazed/base`. This covers a compile error, a `Cargo.lock` change from checking, and the script's `Cargo.lock: re-resolve after rebasing onto <tag>` commit; `Cargo.lock` changes go into `r_tools: add crate with send-to-terminal`, which added the dependencies. Rerun `script/update-datazed --no-build <tag>` after folding.

An error about a missing field or a type mismatch that the source does not explain (the field or type is in the source as the error expects) has twice come from an outdated build artifact, cause unknown. Capture `CARGO_LOG=cargo::core::compiler::fingerprint=info` with the failing command once, then `cargo clean -p <crate>` for the crate it names and run again.

Done when the script ends with "passes the checks", `cargo check -p zed` succeeds, no `Cargo.lock: re-resolve…` commit is left, and `git status` is clean.

## 4. Subset table

`DATAZED.md`'s subset table records which commits apply on their own. Recheck it against the new base:

```sh
git rev-list --reverse datazed/base..data-tools | while read -r c; do
  git merge-tree --write-tree --name-only --merge-base "$c^" datazed/base "$c" >/dev/null 2>&1 && r=clean || r=conflict
  echo "$r $(git log -1 --format=%s "$c")"
done
```

Done when every commit's result matches its "Stands alone?" column, each commit dropped in step 2 has lost its row here and in `conflicts.md`, or the table is corrected in the `DATAZED.md` commit (fixup, as in step 3). The "Common subsets" builds are not rerun here; say so when reporting.

## 5. Build, install and publish

Ask the user to quit DataZed, since the build replaces the installed app, and whether to publish. Then run one of:

```sh
script/update-datazed --build-only
PUSH_REMOTE=<fork> script/update-datazed --build-only --push
```

It installs `/Applications/DataZed.app`, verifies its signature, launches it, quits it after 8 seconds, and fails on a `panic` line in `~/Library/Logs/Zed/Zed.log`. With `--push`, only after the user's yes, it then force-pushes `data-tools`, leased on the fork's branch as this checkout last fetched it, and the `datazed/base` tag.

Done when the script ends with "Installed /Applications/DataZed.app", and, after a push, `git ls-remote <fork> data-tools refs/tags/datazed/base` matches the local `data-tools` and `datazed/base`. Mention to the user that clones of the fork need `git fetch && git reset --hard origin/data-tools`, and offer to delete the fork's `datazed-update/<tag>` and `datazed-update/<tag>-failed` branches if they exist.
