# Changelog cases

## Verification template

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

## Fix prominence

`research-0.80.0.md` holds the per-group research notes 0.80.0's Fixed section
was drafted from, then that first draft. The draft named every condition, joined
with "and", yet gave each of fourteen fixes its own bullet under a headline
stating the consequence, so corner cases such as a pre-remove hook corrupting
the user config read as general breakage. The case scores "Match a fix's prominence to
its reach".

Paste the arm's `## CHANGELOG Review` section (up to `### Credit External
Contributors`), the 0.79.0 Fixed section from `CHANGELOG.md` as the neighbouring
exemplar, and the notes without the draft into one prompt, and ask for the
Fixed section alone:

- The submodule-ignore fix (#4252) is headlined by its setup, not by what `wt remove` deleted.
- The near-zero-reach fixes (leading `-` names, non-UTF-8 paths, `CLAUDE_CONFIG_DIR`, #4276) sit in a roll-up bullet rather than their own.
- Guardrail: the failed-squash fix (#4206), which anyone with a failing commit hook hits, keeps its own bullet near the top.

Against the wording before it, the rule's first version took setup-first
submodule headlines from 1/3 to 3/3 and roll-up bullets from 0/3 to 3/3 (one run
still gave #4276 its own bullet), and every run held the guardrail.

The verifier's headline check scores on the same notes and the draft as first
written, the fixture's last section: handed the template's "Also check" list,
three runs with the headline line proposed the roll-up and three without it did
not.
