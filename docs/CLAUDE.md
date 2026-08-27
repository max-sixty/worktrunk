# Documentation Site

The Worktrunk site is built with Astro and Starlight and published at
https://worktrunk.dev.

## Development

The project hook installs the site dependencies, fetches the demo assets, and
starts the Astro dev server. Find this worktree's URL with `wt list`.

To run it yourself:

```bash
npm --prefix docs install
npm --prefix docs exec playwright install webkit
npm --prefix docs run dev -- --host 127.0.0.1 --port 4321
```

The local production checks are:

```bash
npm --prefix docs run check
npm --prefix docs test
npm --prefix docs run build
npm --prefix docs run test:site
```

`npm run build` clears Astro's content cache, writes `docs/dist/`, and builds
the Pagefind search index. The forced content rebuild is intentional: renderer
plugin changes affect generated asset hashes but are not part of Astro's
content-cache key.

### Verifying changes

Text-only edits need the docs sync test and a production build. Visual changes
also need browser verification with Playwright at desktop and mobile widths.
Check the changed page, one command-reference page, search, the mobile menu,
theme switching, anchor navigation, code copying, wide tables, and demo images.

Always include the local dev-server link when handing off a docs change:

```text
View changes: http://127.0.0.1:<port>
```

## Site architecture

| Path | Responsibility |
|------|----------------|
| `astro.config.mjs` | Starlight integration, navigation, metadata, and code rendering |
| `src/content/docs/` | Canonical documentation Markdown |
| `src/pages/index.astro` | Homepage route; renders the canonical `worktrunk.md` body |
| `src/styles/custom.css` | Worktrunk's visual system and narrow Starlight adjustments |
| `src/plugins/stable-heading-ids.mjs` | Stable public anchor IDs for Markdown headings |
| `src/plugins/worktrunk-terminal.mjs` | Shell syntax, prompts, copy behavior, Clap help roles, and ANSI-derived output styling |
| `src/themes/worktrunk-code.mjs` | Light and dark syntax themes for source and command blocks |
| `src/generated/terminal-styles.json` | Generated site-only span styles from ANSI command snapshots |
| `src/components/Head.astro` | Social metadata, structured data, and analytics |
| `tests/` | Renderer unit tests plus built-site and WebKit mobile checks |
| `public/` | Files served unchanged at the site root |
| `demos/` | VHS sources and scripts for the external demo assets |

The site uses Starlight's own navigation, table of contents, responsive shell,
search, code frames, copy controls, and theme selector. Keep overrides narrow.
Before replacing a Starlight component, check whether configuration or CSS can
express the change. Every full component override becomes an upstream merge
surface.

The visual direction is a warm technical field manual: ivory paper, dark ink,
one orange accent, strong typography, and terminal output as the main visual
material. Avoid generic product-site devices such as gradient headline text,
glass cards, feature-card grids, decorative blobs, and animation without a
clear navigational or explanatory purpose.

## Content and route contract

Public documentation routes are stable. Existing pages should keep their root
paths, for example `src/content/docs/switch.md` is `/switch/`. The homepage
renders `worktrunk.md`; `/worktrunk/` remains available for compatibility and
has a canonical link to `/`.

Use root-relative links in canonical Markdown:

```markdown
[hook templates](/hook/#template-variables)
```

The sync pipeline expands them to full `https://worktrunk.dev/...` URLs for
README and agent-skill copies. Do not add framework-specific link syntax.
`stable-heading-ids.mjs` preserves the site's established anchor scheme, and
`test:site` verifies every built internal page link and fragment.

Images and demos use root-relative paths into `public/`:

```html
<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-switch.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-switch.gif" alt="wt switch demo" width="1600" height="900">
</picture>
</figure>
```

## Documentation sync taxonomy

`cargo test --test integration test_docs_are_in_sync` owns the complete sync
pipeline. It updates generated files and then fails so the changes are visible.
Run it again after reviewing those changes; the second run must pass.

There are three source categories:

1. **Command pages**: `src/cli/mod.rs` is primary for `config`, `hook`, `list`,
   `merge`, `remove`, `step`, and `switch`. The sync test writes the generated
   region in `docs/src/content/docs/{command}.md`, then generates the matching
   `skills/worktrunk/reference/` page.
2. **Non-command pages**: files such as `claude-code.md`, `extending.md`,
   `faq.md`, `llm-commits.md`, `tips-patterns.md`, and `worktrunk.md` are primary
   in `docs/src/content/docs/`. The sync test derives the skill copy.
3. **Skill-only pages**: files such as `shell-integration.md` and
   `troubleshooting.md` are primary in `skills/worktrunk/reference/` and have no
   site page. When adding one, add a `linguist-generated=false` entry to
   `.gitattributes`.

Never hand-edit a generated mirror.

### Command-page generation

Each command page keeps YAML frontmatter outside one generated region:

```markdown
---
title: "wt list"
description: "List worktrees and their status."
sidebar:
  order: 11
---
<!-- ⚠️ AUTO-GENERATED from `wt list --help-page` — edit src/cli/mod.rs to update -->

[generated content]

<!-- END AUTO-GENERATED -->
```

The bare close marker is paired with the open marker using a non-greedy match.
`test_no_nested_auto_generated_markers` enforces the required invariant: an
auto-generated region may not contain another auto-generated region.

Each command has three pieces in `src/cli/mod.rs`:

| Piece | Source | Purpose |
|-------|--------|---------|
| Definition | First `///` line | Short text for command lists |
| Subdefinition | Second `///` line, when useful | Context below the terminal help header |
| Full guide | `after_long_help` | Mental model, workflow, examples, and reference links |

Terminal help displays the full guide after the options. The web page combines
the definition and subdefinition as its lead, followed by the full guide. Do
not repeat the lead at the start of `after_long_help`.

Link text must still make sense when terminal help removes the URL. Prefer
``[`wt merge`](/merge/)`` or a descriptive phrase over a bare destination
heading.

Config examples between `USER_CONFIG_START` / `USER_CONFIG_END` and
`PROJECT_CONFIG_START` / `PROJECT_CONFIG_END` also generate commented TOML
files. Put prose before a code block instead of adding standalone TOML comment
lines that would become double-commented. End-of-line comments are fine.

After changing help text, refresh both generated pages and help snapshots:

```bash
cargo test --test integration test_docs_are_in_sync
cargo insta test --accept --test integration -- test_help
```

### Snapshot examples

A command placeholder in `src/cli/mod.rs` expands from an integration snapshot:

````markdown
<!-- wt list -->
```console
$ wt list
```
````

The mapping lives in `tests/integration_tests/readme_sync.rs`. The generated
site page uses the real command plus ANSI-stripped output in one `console`
fence. This keeps the Markdown readable on GitHub and lets the site renderer
add prompts and command-only copy behavior without changing the source. The
same sync pass writes `src/generated/terminal-styles.json` from the snapshot's
ANSI spans, so the website preserves the CLI's exact semantic colors and text
attributes while every portable Markdown surface remains plain text.

To update an example:

1. Change the test setup that owns the snapshot.
2. Run the focused integration test.
3. Accept the snapshot with `cargo insta accept`.
4. Run `test_docs_are_in_sync` twice, reviewing the first run's edits.

### Code-block convention

Use ordinary fenced Markdown. There are no template shortcodes or encoded
command parameters.

- Use `bash` for commands a reader can copy as a complete recipe.
- Use `console` when commands and captured output share a block. Prefix command
  lines with `$ `; leave output unprefixed.
- Use the actual data language (`toml`, `json`, `yaml`, and so on) for files and
  structured output.

Example:

````markdown
```console
$ wt switch --create feature-auth
✓ Created branch feature-auth from main
```
````

Starlight and Expressive Code create the frame and copy button. The Worktrunk
plugin highlights `console` commands as Bash, renders `$ ` as a prompt, and
makes mixed blocks copy only their commands. Comment lines and blank recipe
separators remain copyable; captured output does not. Snapshot-backed output
gets its exact ANSI roles from the generated style manifest, while hand-written
output uses a conservative marker fallback. Generated Clap help fences carry
the `wt-command-reference` marker, which the plugin expands into semantic
command, option, value, and metadata roles. Committed Markdown must remain
useful without the plugin.

### Web-only post-processing

`post_process_for_html()` in `src/help.rs` handles the few semantic differences
between terminal and site output: experimental badges, CI color labels, demo
figures, and a linked issue-report phrase. Do not route general Markdown
through it and do not add pre-rendered ANSI HTML.

### Subdocument expansion

Use a subdocument placeholder to include a subcommand as a section of its
parent page:

```markdown
<!-- subdoc: create -->
```

The generator raises the subcommand heading levels and appends its command
reference. The comment is invisible in terminal help.

## Template examples

Every Worktrunk template expression shown in documentation needs a matching
test in `tests/integration_tests/doc_templates.rs`. This is what catches parser
and operator-precedence changes before they make an example misleading.

## Demo assets

Large GIFs and rendered social cards live in the separate
`max-sixty/worktrunk-assets` repository. `task fetch-assets` copies published
assets to `docs/public/assets/`, which is gitignored. The publish workflow does
the same before building.

To regenerate and publish demos:

```bash
./docs/demos/build docs
./docs/demos/build social
task publish-assets
```

See `docs/demos/CLAUDE.md` for timing, terminal setup, validation, and recording
guidance.

Social-card SVG sources remain in `docs/public/`:

- `social-card.svg` is the 1200 by 630 Open Graph source.
- `github-social-card.svg` is the 1280 by 640 repository-preview source.

Build their PNG outputs with `task build-social-cards`, then publish them with
`task publish-assets`. `src/components/Head.astro` points social metadata at
`/assets/social/social-card.png`.
