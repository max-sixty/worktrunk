# Command development

## Interactive debugging

Run the current binary in a temporary test repository with a real PTY. Use tmux-cli when available, capture terminal output, and inspect -vv logs when behavior depends on timing or shell state. Avoid treating a piped command as proof of interactive behavior.

## Adding a CLI command

Define the Clap interface and help in src/cli/mod.rs, implement the command here, and wire dispatch in src/commands/mod.rs. Update generated command pages with test_docs_are_in_sync and refresh help snapshots with test_help. See docs/AGENTS.md for the sync contract.

Worktree-naming arguments accept a branch, with a path as an alias; route them through Repository::resolve_worktree. Use the established target terminology for merge destinations. Keep --dry-run read-only, and make -v add detail rather than change behavior.

## CLI help and command pages

The first Clap doc line is the short command description. The second may add context under the terminal header. after_long_help owns the guide and examples, shared by terminal help and generated site pages. Do not repeat the short description there. Describe what the command does without presuming why the user runs it; keep implementation detail below common workflows. Link to dedicated pages for cross-command concepts.
