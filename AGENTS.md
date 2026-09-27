# Worktrunk development guidelines

## Quick start

Run cargo run -- hook pre-merge --yes for the project test and lint gate. Claude Code web first runs task setup-web; Codex Cloud uses .codex/cloud.sh. Test and coverage recipes live in tests/AGENTS.md.

## Project Status

Worktrunk is a CLI with a growing user base. New commands, flags, and config keys are product decisions. Config formats and CLI arguments are protected interfaces; internal APIs and output formatting are flexible. MSRV is latest stable minus one.

## Terminology

- main worktree: the original Git directory; bare repos have none
- linked worktree: one created by git worktree add
- primary worktree: the home worktree, or the default-branch worktree in a bare repo
- default branch: the branch named main, master, or equivalent
- target: a merge, rebase, or push destination, never a worktree

## Skills

Load writing-user-outputs before changing visible CLI text, running-tend for Tend CI, and release for a release. Project-local skills live in .claude/skills/, linked for Codex from .agents/skills/.

## Worktree Model

Each worktree maps to one branch. Worktree arguments resolve branch first through Repository::resolve_worktree; a path aliases its checkout, including detached or duplicate-branch checkouts. Never retarget an existing worktree to a different branch; create, switch, or remove instead. The experimental wt step promote is the sole branch-exchange operation.

## Documentation

Behavior changes require documentation updates. src/cli/mod.rs is primary for command help and generated command pages; docs/AGENTS.md explains sync. Check that --help matches behavior. Run cargo test --test integration test_docs_are_in_sync, then refresh help snapshots when help text changes.

Docs describe behavior for a user, not the history of a fix. A fix that makes a feature work the way a reader already assumed needs no new doc sentence; do not add lines announcing that an edge case now works. LLM-generated changes add these often, so cut them in review.

## Plugin Layout

The loader paths, generated skills mirror, and cross-tool hook requirements are in plugins/worktrunk/AGENTS.md.

## Data Safety

Prefer a clear failure over losing untracked files, uncommitted changes, or user data. Make cleanup a separate action, never a destructive side effect of an unrelated command. Force removal requires explicit consent. Git operations should use their failing variant on races: reset --keep and checkout --merge instead of destructive alternatives.

Be conservative across the gap between a safety check and the operation. If files appear before cleanup, fail rather than force-remove them.

Full-file rewrites of user-owned or externally owned files use utils::write_atomically; creation of an observed-missing file uses write_new_atomically. Shell rc additions append under the installer lock; removal rewrites atomically. A user-named --output path is an intentional replacement. Regenerable caches and diagnostic reports may use ordinary writes. See the write helper contracts for symlink and mode behavior.

Git itself may overwrite an ignored destination file when a tracked file arrives, and core.fsmonitor controls Git's final dirty-worktree gate. The inventory is in the FAQ sections “What files does Worktrunk create?” and “What can Worktrunk delete?”

## Command Execution Principles

### All Commands Through shell_exec::Cmd

External commands use shell_exec::Cmd for logging and timing; Git generally uses Repository::run_command or WorkingTree::run_command. Use run for capture, stream for live output, delayed_stream for slow setup, and pipe_into for pipelines. In-process spawns outside Cmd use a CommandTrace guard; its contract is in src/trace/emit.rs. Detached background children and interactive helpers are deliberately untraced.

### Git-Discovery Env Vars Follow Who Chose the Cwd

A child whose cwd is a Worktrunk-chosen worktree must scrub inherited Git discovery variables before running, including hooks and worktree-local plumbing. Repo-level commands and commands in the user's own context keep inherited values. The site classification is in scrub_git_discovery_env_vars in src/shell_exec.rs.

### Real-time Output Streaming

Show the first local result promptly. Stream later output line by line. Network detail may arrive after the first frame, but must not block it.

### Structured Output Over Error-Message Parsing

Use exit codes and machine-readable output. For example, distinguish git merge-base results by status code and parse git status --porcelain=v2 -z, rather than matching localized prose.

### Plumbing for Output Worktrunk Consumes

Git output that Worktrunk parses, caches, renders, or prompts with uses plumbing, with submodule behavior pinned through PlumbingDiff::args and rendered diffs through PreparedDiff::capture. Output displayed as Git's own view, such as wt step diff, may stay porcelain.

### Immutable Ids Over List Positions

Do not carry stash@{n} or another position through a mutation window. Capture an object id or derive the position from a stable id immediately before use.

### Network Access

Worktrunk touches the network only when the user asks for it. The first Repository::default_branch() is the bounded detection exception: it may use git ls-remote and cache the result; timeout falls back to local inference without caching. No other detection helper adds a wire fallback. A synchronous shell-prompt hot path never touches the network; wt list statusline may fetch CI status because its host consumes it asynchronously. The picker can stream forge results into visible rows and cancel them when the user leaves; a run-to-completion command waits for its last request even after first paint.

### Signal Handling: Ctrl-C Cancels the Current Command

When a child is interrupted, every foreground loop stops before its next step, including Warn hook pipelines and worktree loops. Use err.interrupt_signal() from ErrorExt and propagate WorktrunkError::Interrupted. Capture mode treats SIGINT and SIGTERM as interrupts; other child signals remain visible failures. See src/shell_exec.rs and src/commands/command_executor.rs for signal normalization and rendering.

### Project Commands Run Only After Approval

Project hooks, aliases, and --execute commands are arbitrary code. Gate them through Approvals before running. For an operation that mutates state between approval and execution, select once into ApprovedHookPlan and execute only that plan; do not reread config to choose new commands. A noninteractive context runs only an already approved subset. The contract is in src/commands/hook_plan.rs.

### Background Hook Pipelines Run Concurrently, Per Source

A post-hook batch spawns one detached pipeline per source. A source can run indefinitely, so do not serialize independent sources. HookAnnouncer marks an anchor worktree removed before flush, preventing a hook from running in a path that no longer resolves to that worktree. See src/commands/process.rs.

## Coverage

codecov/patch is a merge gate even if GitHub marks it optional. Fix real gaps; do not bend the design to a predicted percentage. A failing patch check requires explicit approval before merge. Check runs appear after the coverage job, not in commit statuses. Investigation commands are in tests/AGENTS.md.

## Code Quality

### Use Existing Dependencies

Check Cargo.toml and installed APIs before writing a utility. Prefer path_slash for path normalization, shell_escape for shell quoting, color_print for ANSI styling, and MiniJinja's undeclared_variables for template variables. Let dependencies own their own rules.

Name pure accessors as bare nouns, erroring preconditions require_*, network calls fetch_*, and file loads load_*; avoid get_*.

### System Docstrings

Complex state machines and cross-module coordination need a module-level spec describing purpose, decisions, and invariants. Keep it current as the system changes.

Keep test helpers out of library code. Use plain multiline string literals, without continuation escapes. Do not hide unused code with allow(dead_code).

## Error Handling

Use anyhow context for I/O and child command failures, and bail! for business-rule failures. Functions returning Result use ? or explicit errors, not expect or unwrap.

## Config Deprecation

Migrate deprecated TOML before deserialization in src/config/deprecation.rs; never silently drop a key. Each DEPRECATION_RULES row uses one idempotent migrate-and-report function for detection and rewrite. Warning rules fire exactly when wt config update changes the file, and what an update writes must load without deprecation or unknown-field warnings. The module spec and test_warning_fires_iff_update_changes own the details.

## Releases

Use the release skill. Release changelog entries come from commits since the last tag; feature PRs leave CHANGELOG.md untouched.
