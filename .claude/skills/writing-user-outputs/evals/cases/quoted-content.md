---
keep: ^DECLINE$
drop: ^PROMOTE$
---

The saved survey proposes: Keep quoted output visible without color.
Before: Without ANSI background color, quoted output looks like ordinary indented text: "  git error".
After: Use a visible gutter "┃ git error" while keeping the same two-column allocation.
Purpose: Quoted Git errors, help examples and hook previews need a visible boundary in logs and pipes.
Extraction: Small, independent. Add GUTTER_BAR to the shared Bash/TOML/text formatters and regenerate snapshots.
Source-established, not a newly executed usability reproduction. No report of misread content is supplied.

The existing GUTTER style sets a BrightWhite background. The proposed formatter changes are:

```diff
- format!("{gutter} {gutter:#} {line}")
+ format!("{gutter}{GUTTER_BAR}{gutter:#} {line}")
```

The Bash and TOML formatters make the same replacement of the background-colored blank with the literal `┃`. It appears in colored output as well as plain output. ANSI stripping leaves all quoted words, line breaks and the existing two-space indentation intact before the change.
