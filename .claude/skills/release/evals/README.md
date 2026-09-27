# Changelog-verification cases

Four entries from a shipped release, three of which reached users wrong. They
score the "MANDATORY: Verify Each Changelog Entry" prompt template: run it as a
subagent prompt and check which of the three it reports.

The template reads the top section of `CHANGELOG.md` on disk, so run the arm in a
checkout at `v0.77.0`, where that section is the 29 entries these cases come
from. Run the template unedited: pointing it at the section some other way scores
a prompt nobody ships, and the section it would otherwise read is whatever
release is in progress.

```bash
git worktree add /tmp/wt-eval-0.77.0 v0.77.0   # remove it afterwards
```

That checkout carries this skill at `v0.77.0` too, whose template is the one
being replaced. Pass the arm's template as the prompt and withhold the `Skill`
tool, so the run cannot load the old one off disk instead.

Offer the run the notes from the per-group agents as a map, the way a release
does. Handed only these four entries every wording found all three, so what the
cases measure is attention spread across 29 entries, a missing-entries sweep,
and the attribution checks.

None of them has its truthmaker wholly outside the commit, and that is hard to
fix here: this repo's commits routinely restate the surrounding facts in added
comments and doc lines, so the falsifying text usually arrives in the diff. Read
the rows as scoring how much of the diff the verifier reads, not whether it goes
looking beyond one.

| Case | Entry | The claim | What the source says |
|------|-------|-----------|----------------------|
| Before-state, with a confirming-looking diff | `wt list --format=json` gains a `marker` field | "`state` no longer carries `detached`" | `git show 981a207e2^:src/commands/list/model/state.rs` has no `Detached` variant — the PR adds it. The diff's no-op arm for `Detached` reads like the removal of a case that used to emit something. |
| Fact in the diff, in an added comment rather than the code | `wt config state marker` set and clear no-op | "an agent plugin's hooks failed on every event" | Every shipped marker hook ends `\|\| true` (`git grep 'state marker' v0.76.0 -- plugins hooks`), so the hook exited 0. `29a611b7c` says so in a comment it adds beside the fix, so a verifier that reads the whole diff has the contradiction without leaving it. |
| Non-headline clause, settled in the diff | The FAQ lists the files Worktrunk writes | "for Claude Code, Codex, OpenCode, Pi, and Gemini" | The FAQ's new table has three rows. Gemini appears nowhere; Codex appears only in the line saying its install writes nothing. |
| Guardrail | LLM commit messages keep diffs for quoted paths | all of it | Accurate. A run that calls this entry wrong has gone too far. |
