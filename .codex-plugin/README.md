# Worktrunk Codex Plugin

This plugin packages Worktrunk guidance and activity tracking for Codex.

Install through Worktrunk:

```console
$ wt config plugins codex install
```

Then open `/plugins` in Codex and install Worktrunk from the Worktrunk marketplace.

The install command configures the marketplace; it does not install the plugin directly. To remove the marketplace entry later, run:

```console
$ wt config plugins codex uninstall
```

Uninstall leaves any already-installed Worktrunk plugin and the global `codex_hooks` feature unchanged.

The plugin provides:

- Worktrunk skill documentation for configuring hooks, LLM commits, shell integration, and parallel worktree workflows
- Codex lifecycle hooks that set `wt list` markers to show active sessions

Codex does not expose Claude Code's `WorktreeCreate` and `WorktreeRemove` hook events, so Codex users should use `wt switch --create` and `wt remove` directly for worktree lifecycle management.
