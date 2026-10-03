# Output design eval

The quoted-content case replays the stale-survey recommendation that treated
loss of a colored gutter as a defect and proposed a literal bar. It scores the
extraction decision, with verified formatter evidence supplied in the case.
It does not measure whether a new visual design would improve usability.

Run with the personal improving-instructions runner:

```sh
uv run --script ~/.claude/skills/improving-instructions/evals.py .claude/skills/writing-user-outputs/evals --agent codex
```

Pass a full saved skill through `--skill` to compare wording. Inspect the saved
responses as well as the first-line verdicts.
