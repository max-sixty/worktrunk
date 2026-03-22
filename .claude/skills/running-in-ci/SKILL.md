---
name: running-in-ci
description: CI environment rules for GitHub Actions workflows. Use when operating in CI — covers security, CI monitoring, comment formatting, and investigating session logs from other runs.
metadata:
  internal: true
---

# Running in CI

Load `/cd-running-in-ci` for generic CI environment rules (security, comment
formatting, shell quoting, session log analysis, grounded analysis). This file
adds worktrunk-specific CI monitoring.

## CI Monitoring — Additional Required Checks

After required checks pass, poll `codecov/patch` separately — it is mandatory
despite being marked non-required. Use a polling loop (up to ~5 minutes) since
codecov often reports after the required checks finish:

```bash
for i in $(seq 1 5); do
  CODECOV=$(gh pr checks <number> 2>&1 | grep 'codecov/patch' || true)
  if echo "$CODECOV" | grep -q 'pass'; then
    echo "codecov/patch passed"; exit 0
  elif echo "$CODECOV" | grep -q 'fail'; then
    echo "codecov/patch FAILED"; exit 1
  fi
  sleep 60
done
echo "codecov/patch not reported after 5 minutes"
exit 1
```

If codecov fails, investigate with `task coverage` and
`cargo llvm-cov report --show-missing-lines | grep <file>`.

Report completion only after all required checks **and** `codecov/patch` pass.

CI runs on Linux, Windows, and macOS.

## Session Log Paths

Session log artifact paths follow the pattern:
`-home-runner-work-worktrunk-worktrunk/<session-id>.jsonl`

## Applying GitHub Suggestions

Apply the literal suggestion only — change the lines it covers, nothing more.
If surrounding lines also need updating, note that in your reply.
