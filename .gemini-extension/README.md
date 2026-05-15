# Worktrunk Extension for Gemini CLI

Activity tracking for [Worktrunk](https://worktrunk.dev). Sets per-branch status markers in `wt list` so you can see which worktrees have active Gemini CLI sessions.

| Event | Marker | Meaning |
| --- | --- | --- |
| `BeforeAgent` | 🤖 | Agent is working |
| `AfterAgent` | 💬 | Agent finished; waiting for input |
| `SessionEnd` | — | Marker cleared |

Markers are stored in git config (`worktrunk.status.{branch}`) and surface in `wt list` output, useful when running multiple agent sessions in parallel.

## Install

```bash
gemini extensions install <github-url>
```

Requires `wt` (or `git-wt.exe` on Windows) on `PATH`. The hook calls `wt config state marker` and ignores failures so a missing binary will not break the session.
