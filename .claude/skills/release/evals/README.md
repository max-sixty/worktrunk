# Changelog-verification cases

Four entries from a shipped release, three of which reached users wrong. They
score the "MANDATORY: Verify Each Changelog Entry" prompt template. Run the template as a subagent prompt over the section below and check
which of the three it reports.

Both the old and the current wording found all three when handed only these four
entries, so run the template over the whole section, with the earlier per-group
notes offered as a map. The cases measure attention spread across 29 entries, a
missing-entries sweep, and the attribution checks.

```bash
git show v0.77.0:CHANGELOG.md | awk '/^## 0\.77\.0/{f=1} /^## 0\.76\.0/{exit} f'
```

| Case | Entry | The claim | What the source says |
|------|-------|-----------|----------------------|
| Before-state, with a confirming-looking diff | `wt list --format=json` gains a `marker` field | "`state` no longer carries `detached`" | `git show 981a207e2^:src/commands/list/model/state.rs` has no `Detached` variant — the PR adds it. The diff's no-op arm for `Detached` reads like the removal of a case that used to emit something. |
| Truthmaker outside the diff | `wt config state marker` set and clear no-op | "an agent plugin's hooks failed on every event" | Every shipped marker hook ends `\|\| true` (`git grep 'state marker' v0.76.0 -- plugins hooks`), so the hook exited 0. `wt` printed a git error and exited 1. |
| Non-headline clause, settled in the diff | The FAQ lists the files Worktrunk writes | "for Claude Code, Codex, OpenCode, Pi, and Gemini" | The FAQ's new table has three rows. Gemini appears nowhere; Codex appears only in the line saying its install writes nothing. |
| Guardrail | LLM commit messages keep diffs for quoted paths | all of it | Accurate. A run that calls this entry wrong has gone too far. |
