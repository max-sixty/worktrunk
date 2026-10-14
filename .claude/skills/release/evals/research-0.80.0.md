# 0.80.0 commit research (fixture)

The per-group research notes the 0.80.0 release drafted its Fixed section from, verbatim apart from scratch paths. See README.md, "Fix prominence".

## Fix commits

Most of these PRs fix narrow edge cases, but three matter to ordinary users, and one of them changes existing behaviour:

- **#4206 fixes a failed squash that rewound the branch.** Any failing git commit hook or signing failure during `wt merge` or `wt step squash` used to leave the branch at the merge base.
- **#4206 also changes what git commit hooks see.** They now run on a detached HEAD, so a hook that reads the branch name gets `HEAD`.
- **#4252 fixes silent data loss in `wt remove`.** It deleted uncommitted submodule work whenever a submodule ignore setting was on.

"Repro" below means I ran the v0.79.0 binary against the current debug build (`v0.79.0-62-g4b8e6f986`) in `mktemp -d` scratch repos, with isolated config and git 2.55 on macOS. "Code" means I read the diff and the old file.

## A. Squash and commit correctness

**#4206 — squash commits on a detached HEAD** · d65b1a07f · [PR](https://github.com/max-sixty/worktrunk/pull/4206)
- **Old:** the squash ran `git reset --soft <merge-base>`, then `git commit`. If the commit failed, the branch was left at the merge base with all its commits collapsed into staged changes. The command reported `✗ Failed to create squash commit`. The work could be recovered from ORIG_HEAD or the reflog.
- **New:** the branch keeps its commits until the squash commit exists, so a failure changes nothing.
- **Conditions:** `wt merge` with squash (the default) or `wt step squash`, and 2+ commits (or 1 commit plus staged changes), and `git commit` failing. The commit fails when a git `pre-commit`/`commit-msg` hook rejects it (the pre-commit framework, husky, lefthook) or signing fails.
- **Repro:** a `.git/hooks/pre-commit` that exits 1.
  - v0.79.0: `main..feature` is empty and `a`/`b` are staged.
  - New: both commits remain on the branch.
- **Who hits it:** many people. Hook frameworks are common and formatter hooks routinely fail.
- **Behaviour change, not in the subject:**
  - Git's `pre-commit`, `commit-msg` and `post-commit` hooks now see a detached HEAD. In the repro `git rev-parse --abbrev-ref HEAD` printed `feature` on v0.79.0 and `HEAD` now.
  - Hooks that derive a ticket prefix from the branch name will silently stop working for squash commits.
  - The `wt step squash` help text now says the commit is made on a detached HEAD.
- **Bundled fix (commit signing):** the check for whether to sign now asks git directly.
  - Old: a key written as bare `gpgsign` (no `= true`) or set in worktree-scoped config was read as false, so `wt merge --no-ff` produced an unsigned merge commit.
  - Repro: with a fake gpg program, v0.79.0 merged unsigned with 0 gpg calls. The new build called gpg, which fails as intended with the fake program.
  - Who hits it: rare.
- **Bundled fix (empty-squash check):** a squash whose only change is a submodule pointer hidden by `submodule.<name>.ignore` is now committed. Before, it reported "No changes after squashing" (code).

**#4192 — staged-change check uses plumbing** · efa59ee8c · [PR](https://github.com/max-sixty/worktrunk/pull/4192)
- **Old:** the check was `git diff --cached --quiet` in `wt step commit`, `wt merge`'s commit step and squash, and any git error counted as "has changes".
- **Condition that reaches users:** `submodule.<name>.ignore=all` and the only staged change is a submodule pointer.
  - Repro: v0.79.0 gave `✗ Nothing to commit`. New gives `✓ Committed changes`.
  - In squash (code): with 1 commit, that staged pointer was treated as "already a single commit" and left out.
- **Not reachable from the CLI:** the PR's `diff.relative` case, where the check runs from a subdirectory. `wt step commit` run from a subdirectory with `diff.relative=true` worked on v0.79.0 because wt runs git at the worktree root (`[repo]` in the `-vv` trace).
- **Who hits it:** very few.

**#4251 — merge decides whether to commit using the `--stage` mode** · d65adb038 · [PR](https://github.com/max-sixty/worktrunk/pull/4251)
- **Old:** `wt merge --no-squash` with existing commits aborted with `✗ Nothing to commit` whenever the worktree was dirty but the stage mode would stage nothing. Main was not updated.
- **Conditions:** `--no-squash` (or squash disabled in config), and either of:
  - `--stage=tracked` or `none` with only untracked files. Repro'd for both.
  - Dirty submodule contents under the default `--stage=all`. Repro'd.
- **New:** it merges. Without `--no-remove`, removing the worktree afterwards still refuses because of the untracked file (repro).
- **Who hits it:** few.

## B. Data loss: submodule ignore settings hid changes from `wt remove`

**#4252** · c2feef082 · [PR](https://github.com/max-sixty/worktrunk/pull/4252)
- **Old:** the dirty check honoured the user's submodule ignore setting. wt then ran `git worktree remove --force` (always required when submodules are present), which deleted the worktree including uncommitted submodule changes.
- **Conditions:** the worktree has an initialized submodule, and any of these settings is on:
  - `submodule.<name>.ignore` = `all` or `dirty`, including when committed in `.gitmodules`
  - `diff.ignoreSubmodules` in git config

  and the submodule has modified files (or, under `ignore=all`, unpushed commits).
- **Repro:** I reproduced all of `ignore=all`, `ignore=dirty` via `.gitmodules`, `diff.ignoreSubmodules=dirty`, and an unpushed submodule commit under `ignore=all`. v0.79.0 printed `✓ Removed feature worktree & branch` and the file was gone. New refuses with `✗ Cannot remove worktree: feature has uncommitted changes / M sm`.
- **Other paths (code):** the same check guards removal in `wt merge`, `wt step prune` and `wt step promote`.
- **Who hits it:** a minority (submodule users with an ignore setting), but it is silent data loss.

## C. Stale and orphan worktree removal

**#4226 and #4228** · 7193c0944, 3de0b5f29 · [#4226](https://github.com/max-sixty/worktrunk/pull/4226), [#4228](https://github.com/max-sixty/worktrunk/pull/4228)

#4228 supersedes #4226 for users, so the net change from v0.79.0 is what matters. The trigger is a worktree whose `.git` file was deleted while its directory remains. macOS's `$TMPDIR` cleanup does this; so does a copy that skipped dotfiles.

- **Detached stale entry:**
  - v0.79.0: `wt step prune` failed with `✗ pruning stale worktree for (detached …) fatal: validation failed` and every later prune in that repo failed the same way. Repro.
  - New: the entry is unregistered.
- **Branch stale entry:**
  - v0.79.0: `wt remove feature` printed `✗ Worktree directory missing for feature / ↳ To clean up, run git worktree prune`, and prune silently skipped it. Repro.
  - New: `✓ Pruned stale worktree & removed branch feature (same commit as main, _)`. The directory and its files stay. Repro.
- **New safeguard:** a stale entry whose index holds staged changes, or with a rebase/merge/bisect in progress, is kept.
  - v0.79.0 pruned it along with its index. Repro: staged file, then `rm -rf` of the directory.
  - Now prune skips it silently, and `wt remove` says `✗ Worktree for staged is stale but holds staged changes`, with hints for `-f` or `git worktree repair`. Repro.
- **Bundled text and behaviour changes:**
  - Error: "Worktree directory missing for X" is now "Worktree for X is stale; its directory or .git is gone". The hint is now `git worktree repair <path>` instead of `git worktree prune`.
  - The `⊟` legend now reads "Prunable (worktree directory or its `.git` gone)".
  - The prune help and the FAQ have new text.
  - The picker's branch-only removal now also unregisters the stale entry.
- **Who hits it:** macOS temp-dir worktrees, occasionally.

**#4233 — orphan worktree (`git worktree add --orphan`) before its first commit** · 2a4aa28c2 · [PR](https://github.com/max-sixty/worktrunk/pull/4233)
- **Old** (all repro'd):
  - `wt remove fresh` removed the worktree but printed a bogus `↳ Branch unmerged; to delete, run wt remove -D fresh`.
  - `--foreground` ended with `✗ git rev-parse --verify … failed / fatal: Needed a single revision`.
  - On a stale orphan it printed `○ Branch fresh3 retained; has unmerged changes` plus the `-D` hint.
- **New:** clean `✓ Removed fresh2 worktree` / `✓ Pruned stale worktree for fresh3`.
- **Who hits it:** very few.

## D. `--separate-git-dir` repositories

**#4236** · a56e6d991 · [PR](https://github.com/max-sixty/worktrunk/pull/4236)
- **Old:** `{{ repo_path }}` resolved to the parent of the git store, so new worktrees landed outside the project. `wt list` didn't mark main as current. `wt remove` failed with `fatal: not a git repository`.
- **Conditions:** the repo was created with `git init/clone --separate-git-dir`, and `git worktree repair` has been run. Repair writes the `<store>/gitdir` backlink the fix reads.
- **Repro:**
  - With repair: v0.79.0 created `…/tmp.X.probe` and `wt remove` failed. New created `…/work.probe`, `wt list` shows `@ main`, and remove works.
  - Without repair: both versions are broken identically, as the PR states.
- **Who hits it:** almost no one.

## E. Branch and remote names starting with `-`

- **#4253** (96a00efbe, [PR](https://github.com/max-sixty/worktrunk/pull/4253)): the background removal fallback, used when renaming the worktree directory fails (e.g. across filesystems), ran `git branch -D <branch>`. A `-x` branch was parsed as an option, so the worktree went and the branch stayed. It now uses `-- <branch>` (code).
- **#4267** (2014eb3f1, [PR](https://github.com/max-sixty/worktrunk/pull/4267)): a remote named `-x` broke remote-HEAD lookup, the default-branch `ls-remote` fallback and remote-URL lookup. Separators were added (code).
- **#4277** (51f9a7cb1, [PR](https://github.com/max-sixty/worktrunk/pull/4277)): in a bare repo whose default branch starts with `-` and has no worktree checked out, `git show -x:.config/wt.toml` failed quietly and all project config (hooks, aliases) vanished without an error (code).

Git's normal commands refuse branch names starting with `-` (only plumbing like `update-ref` creates them), and dash-leading remote names need `git remote rename -- origin -x`. Near-zero users; I didn't reproduce these.

## F. Non-UTF-8 filenames (Linux only; macOS's filesystem rejects them, so not repro'd)

- **#4257** (2710f7a82, [PR](https://github.com/max-sixty/worktrunk/pull/4257)): `wt step copy-ignored` and `wt step promote` decoded ignored paths lossily, so the lookup missed the real file.
  - An ignored file with such a name was silently skipped: `copy_leaf` treats a missing source as vanished and logs only at debug level.
  - A top-level directory with such a name errored with "reading directory".
  - `promote` left the entry unstaged, so it wasn't swapped.
  - JSON output now shows the name lossily.
- **#4259** (98533f377, [PR](https://github.com/max-sixty/worktrunk/pull/4259)): `wt step push`'s conflict check could collapse two different names (e.g. `collision-\xFE` and `collision-\xFF`) into the same string. That produced a false conflict that refused the push. It also merged two duplicate status parsers into one.

Near-zero users for both.

## G. Claude Code config dir and `wt config show`

- **#4262** (48d92561e, [PR](https://github.com/max-sixty/worktrunk/pull/4262)): Windows only, when `HOME` differs from `USERPROFILE`. The PR's evidence sets a POSIX-style `HOME`.
  - Old: `install-statusline` reported `✓ Statusline configured` but wrote under `HOME`, where Claude Code (which uses `USERPROFILE`) never looks. `wt config show` reported a correctly installed statusline as "not configured".
  - Unix behaviour is unchanged (code).
  - My inference, not verified: Git Bash may pass `HOME` to native programs already converted to `C:\Users\…`. If so, the realistic population is Windows users with `HOME` pointed elsewhere, such as a corporate `H:\` drive. That's small.
- **#4247** (f88b827c4, [PR](https://github.com/max-sixty/worktrunk/pull/4247)): `CLAUDE_CONFIG_DIR` set to an unexpanded literal `~` made `install-statusline` create a directory named `~` in the current directory. The change also expands `~\foo` on Windows (code). Needs the variable set somewhere that doesn't expand `~`, so near zero.
- **#4255** (bbab727d7, [PR](https://github.com/max-sixty/worktrunk/pull/4255)): `wt config show --full` outside a git repo printed only `✗ git rev-parse --git-common-dir failed (exit 128)` and lost the whole report. Repro. It now renders USER CONFIG and DIAGNOSTICS. Anyone running it outside a repo hits this, so it's plausible but low-stakes.


## Perf, docs, and remaining fix commits

Four of the fixes are user-facing and reproduced cleanly against v0.79.0. The five perf PRs add up to two speedups. #4261 is filed as "docs:" but changes what `wt` does. I built each fix repro in a `mktemp -d` scratch repo and ran it with the installed v0.79.0 binary and with a copy of `target/debug/wt` built at HEAD 4b8e6f986. Nothing in the worktree was touched.

## Fixes

**#4281 `wt step diff` counts only the branch's own changes when local `main` is stale** — dacc21a39, https://github.com/max-sixty/worktrunk/pull/4281
- **Old:** the diff base was `merge-base(HEAD, target)`. If the branch was built on `origin/main` and local `main` lagged it, every upstream commit the branch sits on showed up as the branch's own change.
- **New:** the base comes from `span_upstream`, the same rule `wt merge`, `wt step squash` and `wt step rebase` already use. So `wt step diff` now matches what `wt merge` would squash.
- **To hit the old bug:** run `wt step diff` with the default or a local-branch target, **and** that target has an upstream configured, **and** local `main` is behind `origin/main`, **and** the branch forked from the newer `origin/main`. An explicit `origin/main` or a SHA target is unchanged.
- **Reproduced:** 3 upstream commits pushed from a second clone and fetched, local `main` left stale, one feature commit. `wt step diff -- --name-only` printed `feature.txt up1.txt up2.txt up3.txt` on 0.79.0 and `feature.txt` on current.

**#4279 hook-log JSON reports real branch names; two Tips recipes fixed** — 17b100ef3, https://github.com/max-sixty/worktrunk/pull/4279
- **Output change, worth flagging:** in `wt config state logs --format=json`, the `hook_output[].branch` field used to hold the sanitized log directory name. It is now the real branch name, or `null` when no local branch writes to that directory (branch deleted, detached HEAD, or two branches sharing a directory).
  - Reproduced: for `feature/x`, 0.79.0 printed `"branch":"feature-x-x2d"` and current prints `"branch":"feature/x"`.
  - Anyone filtering on the sanitized form (for example the old extending-page `hook-log` alias with `sanitize_hash`) needs to update.
- **Bundled doc fixes:**
  - The "Database per worktree" recipe passed three `KEY=VALUE` pairs to one `vars set` call, and used the key `db_url`. Now three chained `vars set` calls and the key `db-url`.
  - "Monitor hook logs" used `wt config state logs get --hook=…`. Now a `jq` recipe, and the `wtlog` alias became a shell function.
  - The `wt config state logs --help` examples now match the branch exactly instead of using `startswith`/`head -1`, and the help gains a sentence explaining `branch` and `null`.
  - The `hook-log` alias on the extending page was updated to match.
- **Reproduced:** the old recipe commands fail on 0.79.0 and on current alike, so this half is a docs fix:
  - `vars set a=1 b=2` gives `unexpected argument 'b=2'`.
  - `vars set db_url=x` gives `Invalid key "db_url"`.
  - `logs get --hook=…` gives `unexpected argument '--hook'`.

**#4276 removal hooks survive the user config file changing mid-command** — e1cc1295e, https://github.com/max-sixty/worktrunk/pull/4276
- **Old:** the pre-remove and post-remove steps each re-read the user config at hook time. If that read failed, the hooks were skipped silently. That covers pre-remove itself, and post-remove plus post-switch together.
- **New:** both steps use the config snapshot taken at command start.
- **To hit the old bug:** the user config file becomes unparseable after the command starts (an invalid config at startup already fails `wt remove` and `wt merge` up front) **and** removal hooks are configured.
  - In practice this means a pre-remove hook, or a concurrent process, corrupts the config file, and post-remove/post-switch then vanish.
  - The code is the shared removal path in `src/output/handlers.rs`, so it applies to `wt remove`, merge cleanup, prune and picker removal. I tested only `wt remove`.
- **Reproduced:** a pre-remove hook writes `invalid = [` over the config, followed by `wt remove --foreground --force-delete feature`. On 0.79.0 there was no "Running post-remove" line and no marker file. On current the line appears and the marker file is written.
- The PR body says the snapshot is "passed through direct remove, merge cleanup, step prune, and picker removal paths". The merged diff only touches `handlers.rs` (19 lines) plus a test.

**#4274 leaving the picker no longer strands `.merge_file_*` files** — 05d1315d0, https://github.com/max-sixty/worktrunk/pull/4274 (closes https://github.com/max-sixty/worktrunk/issues/4273)
- **Old:** on picker exit, `wt` sent SIGTERM to background git processes, including `git merge-tree` probes. If a probe was inside an external merge driver, git never cleaned up the driver's three temp files.
- **New:** a probe that has already started runs to completion. One that hasn't started still never spawns.
- **To hit the old bug:** a gitattributes `merge=<driver>` is set (for example mergiraf) **and** some branch conflicts with the target so the probe calls the driver **and** you leave `wt switch` (Esc or accept) while the driver is still running. The files landed in the main worktree root as untracked files.
- **Reproduced** with tmux, using the issue's `sleep 3; exit 1` driver and Esc after 1.5s: 0.79.0 left 3 `.merge_file_*` files on each of 2 runs; current left 0 on each of 2 runs.
- **Trade-off (PR body):** an abandoned probe now outlives `wt` for as long as the driver runs.

## Perf: two user-visible speedups

**A. `wt list` and the `wt switch` picker settle faster in repos with many worktrees** (#4286, #4290, #4288, #4284)
- **#4286** — d13e362cd, https://github.com/max-sixty/worktrunk/pull/4286
  - Tasks now run in display order (`par_bridge`), so picker rows fill in from the top down. Detached (mid-rebase) worktrees are also looked up concurrently before the skeleton, instead of one after another.
  - PR body, ~140-worktree repo, 5 runs: time from rows appearing to visible rows dimmed went from 275–1584ms to 150–230ms. `wt list` skeleton median went from 178ms to 103ms.
  - **Costs, per the PR:** the `wt list` total rose 417 to 461ms. With `--full` and 249 `gh` calls, the last CI result arrived at 6168ms against 5549ms on main, so CI results arrive later.
- **#4290** — 4b8e6f986, https://github.com/max-sixty/worktrunk/pull/4290
  - Builds the ref snapshot from branch inventories already cached for the command. `for-each-ref` scans per run drop from 3 to 2 for `wt list` and from ~4.4 to 2 for the picker.
  - PR body, 50 worktrees, 52 local and 103 remote branches, "task pool opens" median: `wt list` 93 to 78ms; picker 120 to 47ms.
- **#4288** — b3530bfb3, https://github.com/max-sixty/worktrunk/pull/4288
  - Picker only: two setup steps move off the path that delays the task pool.
  - The PR estimates ~10ms saved idle and up to ~130ms under load. It has no end-to-end measurement (load average was 55–164).
- **#4284** — a6930b76c, https://github.com/max-sixty/worktrunk/pull/4284
  - macOS only (`#[cfg(unix)]`, builtin fsmonitor enabled). `wt` now checks the daemon's socket before forking `git fsmonitor--daemon start` for each worktree.
  - PR body: saves 100–300ms before the task pool opens in a 128-worktree repo.

**B. Far fewer forge API calls for CI status** (`wt list --full`, statusline, picker)
- **#4289** — 7294e7964, https://github.com/max-sixty/worktrunk/pull/4289
  - The PR/MR lookup is skipped for local branches that were never pushed: no remote has the name, and the push remote's fetch refspec would have recorded it. Applies to every forge.
  - PR body, 239 local branches of which 19 exist on origin, load average ~30: `gh` calls 249 to 25; last CI result median 9.25s to 5.2s; wall median 12.0s to 9.6s.
  - **Bundled help/docs change:** the blank-CI row in `wt list --help` and list.md now reads "Branch never pushed" instead of "No upstream".
  - **Behavior trade-off:** a PR opened from a branch that another clone pushed shows nothing until this clone fetches.
  - `--single-branch`/`--depth` clones and URL push remotes (for example after `gh pr checkout` of a fork's PR) are still queried.
- **#4216** — 325153bb3, https://github.com/max-sixty/worktrunk/pull/4216
  - GitLab only: the project ID is resolved once per command instead of one `glab repo view` per row.
  - The PR's test goes from 5 calls to 1. There is no timing number.

## Docs and user-facing text

- **#4261** (f732cbda7, https://github.com/max-sixty/worktrunk/pull/4261) is a behavior change despite the `docs:` prefix. Changelog-worthy.
  - The recommended Codex command changed: `gpt-5.6-luna`/`low` becomes `gpt-6-luna` with reasoning `none`. It adds `--ephemeral` and a set of `-c` flags that disable features, and drops `system_prompt=''`, which Codex 0.156.1 rejects under `--strict-config`.
  - The command points `model_instructions_file` at `~/.codex/worktrunk-commit-instructions.txt`.
  - Accepting the first-run "Configure codex for commit messages?" prompt now creates that one-character file. It prints new info, success and hint lines, including "Keep … the saved Codex command requires it".
  - The FAQ lists the file among those Worktrunk creates.
  - PR body: the saved command used 13,729 input tokens on a sample query. In the separate comparisons (one-character file, then each disable), input fell 27,890 to 24,371 and 24,572 to 13,721.
- **#4268** (8e4f8535f, https://github.com/max-sixty/worktrunk/pull/4268) corrects the `wt step` help:
  - `--stage=none` squashes commits *plus* what is already staged.
  - `wt step prune` also skips worktrees with uncommitted changes.
  - The PR verified both by hand; I only read the diff.
- **#4212 and #4213** (6521a8218, 56a4bccd0) extend the lede on the README and docs home with "…hooks to automate local workflows & copy-on-write build caches."
- **#4214** (abd7acaa6) narrows the docs-site content column to fit the 99-column terminal examples and regenerates the `wt list --help` examples at 99 columns. Cosmetic.
- **#4219** (1d46bfca7) removes `--no-hooks` from the shipped worktrunk skill's sub-Agent example, with guidance on when to add it back. It affects plugin and skill users; minor.

## Internal only

- **Candidate Internal bullet:** #4245 (650263036) and #4246 (e8b264c01) move project agent guidance into `AGENTS.md`, shared by Codex and Claude Code, and trim it from 2,769 to 380 lines (PR body).
- **Also internal:**
  - #4287 (ebb27e3bf) deletes the nine one-line `CLAUDE.md` forwarders.
  - #4283 (69d02a03a) adds a doc-cruft rule to `AGENTS.md` and tend review.
  - #4207 (5d994f93c) and #4269 (1f71c08dd) are test-only.
  - #4232, #4244 and #4254 (22bd6beba, ce1862450, f4563a279) are comment-only; I confirmed no code lines changed.

## The Fixed section as first drafted

- **A failed squash no longer rewinds the branch**: when `git commit` failed during `wt merge` or `wt step squash` — a pre-commit hook rejecting it, or signing failing — the branch was left at the merge base with its commits collapsed into staged changes. The squash now commits on a detached HEAD and moves the branch only once the commit exists. (Breaking: git commit hooks see `HEAD`, not the branch name, during a squash.) ([#4206](https://github.com/max-sixty/worktrunk/pull/4206), thanks @Bennyjitsu for finding it in [#4193](https://github.com/max-sixty/worktrunk/pull/4193))

- **`wt remove` no longer deletes submodule changes hidden by config**: with `submodule.<name>.ignore` (`all` or `dirty`, including from `.gitmodules`) or `diff.ignoreSubmodules` set, *and* uncommitted changes inside a submodule, the worktree read as clean and was removed with them. `wt merge`, `wt step prune`, and `wt step promote` shared the check. ([#4252](https://github.com/max-sixty/worktrunk/pull/4252), thanks @Duang777)

- **`wt step diff` shows only the branch's changes when local `main` is stale**: with a target that tracks an upstream *and* a branch forked from a newer `origin/main` than local `main`, the diff included every upstream commit in between. It now uses the same base as `wt merge`. ([#4281](https://github.com/max-sixty/worktrunk/pull/4281), thanks @starlightromero for reporting [#3519](https://github.com/max-sixty/worktrunk/issues/3519))

- **Leaving the picker no longer strands `.merge_file_*` files**: with an external merge driver set in gitattributes *and* a conflicting branch, exiting `wt switch` while a conflict probe ran left the driver's temp files in the main worktree. A started probe now runs to completion. Fixes [#4273](https://github.com/max-sixty/worktrunk/issues/4273). ([#4274](https://github.com/max-sixty/worktrunk/pull/4274), thanks @brndnmtthws for reporting)

- **`wt merge --no-squash` merges when the stage mode stages nothing**: with `--stage=tracked` or `none` and only untracked files, or dirty submodule contents, it aborted with "Nothing to commit". ([#4251](https://github.com/max-sixty/worktrunk/pull/4251), thanks @Duang777)

- **Commit and squash see a staged submodule pointer hidden by `submodule.<name>.ignore=all`**: `wt step commit` reported "Nothing to commit", and squash left the pointer out or reported "No changes after squashing". ([#4192](https://github.com/max-sixty/worktrunk/pull/4192), [#4206](https://github.com/max-sixty/worktrunk/pull/4206), thanks @Duang777)

- **Hook-log JSON reports real branch names**: `wt config state logs --format=json` put the sanitized log directory name (`feature-x-x2d`) in `branch`; it now holds `feature/x`, or `null` when no local branch owns the directory. (Breaking: filters on the sanitized form need updating.) ([#4279](https://github.com/max-sixty/worktrunk/pull/4279))

- **Removal hooks run after a pre-remove hook rewrites the user config**: if the config file stopped parsing mid-command, `post-remove` and `post-switch` hooks were skipped silently. They now use the config read at startup. ([#4276](https://github.com/max-sixty/worktrunk/pull/4276), thanks @Duang777)

- **`wt merge --no-ff` signs merge commits whenever git would**: a bare `gpgsign` key, or one set in worktree-scoped config, read as off, so the merge commit went unsigned. ([#4206](https://github.com/max-sixty/worktrunk/pull/4206))

- **`wt config show --full` works outside a git repository**: it printed only a `git rev-parse` failure; it now shows the user config and diagnostics. ([#4255](https://github.com/max-sixty/worktrunk/pull/4255))

- **Repositories created with `--separate-git-dir` resolve their path**: after `git worktree repair`, `{{ repo_path }}` pointed at the git store's parent, so new worktrees landed outside the project and `wt remove` failed. Fixes [#4235](https://github.com/max-sixty/worktrunk/issues/4235). ([#4236](https://github.com/max-sixty/worktrunk/pull/4236), thanks @zengzheqing for reporting)

- **Statusline install finds Claude Code's config directory**: on Windows with `HOME` differing from `USERPROFILE`, it wrote where Claude Code never reads; a literal `~` in `CLAUDE_CONFIG_DIR` created a directory named `~`. ([#4262](https://github.com/max-sixty/worktrunk/pull/4262), [#4247](https://github.com/max-sixty/worktrunk/pull/4247), thanks @hiro-nikaitou)

- **Branch and remote names starting with `-`**: a background removal left such a branch undeleted, a remote named that way broke default-branch detection, and a bare repo with such a default branch loaded no project config. ([#4253](https://github.com/max-sixty/worktrunk/pull/4253), [#4267](https://github.com/max-sixty/worktrunk/pull/4267), [#4277](https://github.com/max-sixty/worktrunk/pull/4277), thanks @Duang777)

- **Non-UTF-8 file names**: `wt step copy-ignored` and `wt step promote` skipped ignored files with such names, and `wt step push` could report a false conflict between two of them. ([#4257](https://github.com/max-sixty/worktrunk/pull/4257), [#4259](https://github.com/max-sixty/worktrunk/pull/4259), thanks @Duang777)
