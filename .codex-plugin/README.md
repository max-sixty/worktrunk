# Worktrunk Codex Plugin

This plugin packages Worktrunk guidance and activity hooks for Codex.

Install through Worktrunk:

```console
$ wt config plugins codex install
```

Then open `/plugins` in Codex and install Worktrunk from the Worktrunk marketplace.

The install command configures the marketplace; it does not install the plugin directly. To remove the marketplace entry later, run:

```console
$ wt config plugins codex uninstall
```

Uninstall leaves any already-installed Worktrunk plugin and global Codex hook feature flags unchanged.

The plugin provides:

- Worktrunk skill documentation for configuring hooks, LLM commits, shell integration, and parallel worktree workflows
- Codex lifecycle hooks that set `wt list` markers to show active sessions when Codex loads plugin-bundled hooks

Codex CLI releases may gate plugin-bundled hooks behind the `plugin_hooks` feature. If markers do not appear after installing the plugin, run `codex features list`; if `plugin_hooks` is `false`, enable it with `codex features enable plugin_hooks`, or copy `hooks/hooks.json` to a normal Codex hook location such as `~/.codex/hooks.json`.

Codex does not expose Claude Code's `WorktreeCreate` and `WorktreeRemove` hook events, so Codex users should use `wt switch --create` and `wt remove` directly for worktree lifecycle management.
