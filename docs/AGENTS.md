# Documentation site

The site uses Astro and Starlight. The project hook installs dependencies, fetches assets, and starts the dev server; wt list shows this worktree's URL. Site checks are npm --prefix docs run check, test, build, and test:site. Before the first test:site run, install its browser with npm --prefix docs exec playwright install webkit. A config change under a running Astro server may require wt hook post-start docs to clear its content cache.

For text changes, run the docs sync test and build. For visual changes, inspect the rendered page at desktop and mobile widths, including affected navigation, code blocks, and theme states. Hand over the running preview.

## Site architecture

Canonical Markdown is in docs/src/content/docs/. The homepage renders worktrunk.md. Keep existing public routes and anchors stable; src/plugins/stable-heading-ids.mjs owns heading IDs, and test:site checks built links. Use root-relative links and asset paths. Starlight owns navigation, search, code frames, copy controls, and theme selection; keep overrides narrow. The design uses warm paper, dark ink, orange accents, and terminal output as the main visual material.

## Documentation sync taxonomy

Run cargo test --test integration test_docs_are_in_sync. It regenerates mirrors and fails if they changed; review the diff and run it again.

- Command pages: src/cli/mod.rs is primary for config, hook, list, merge, remove, step, and switch. The test updates their generated regions in the site and skills/worktrunk/reference/.
- Other site pages: docs/src/content/docs/ is primary and generates the skill copy.
- Skill-only pages: skills/worktrunk/reference/ is primary. Mark new authored pages linguist-generated=false in .gitattributes.
- plugins/worktrunk/skills/ is a generated mirror of repo-root skills/.

Never edit generated regions or mirrors directly. Command-page frontmatter stays outside the generated markers, and generated regions must not nest.

## Command-page generation

The first Clap doc line is the short description; a second line can supply terminal context. after_long_help contains the guide, examples, and links. Terminal help shows it after options, while the site leads with the short description, so do not repeat that lead. Link text must make sense when terminal help removes the URL.

USER_CONFIG_START and PROJECT_CONFIG_START blocks also generate commented TOML. Put explanatory prose before a TOML fence, not as standalone TOML comments that would be commented twice. After help changes, run test_docs_are_in_sync and cargo insta test --accept --test integration -- test_help.

## Snapshot examples

Command placeholders in src/cli/mod.rs expand from snapshots owned by tests/integration_tests/readme_sync.rs. Update the owning test and snapshot, then run the sync test twice. The sync also writes terminal-styles.json from ANSI spans. Keep portable Markdown plain text; src/plugins/worktrunk-terminal.mjs supplies site-only styling and command-copy behavior.

## Code-block convention

Use bash for copyable commands, console for command plus output with a $ prompt, and the actual language for files. Put a file-path comment first when valid, or a title attribute on the fence. Committed Markdown must remain understandable without the site renderer.

## Template examples

Every documented Worktrunk template expression needs a matching test in tests/integration_tests/doc_templates.rs.

## Demo assets

Large GIFs live in max-sixty/worktrunk-assets and are fetched into gitignored docs/public/assets/. docs/demos/AGENTS.md covers recording and validation. SVG social-card sources remain in docs/public/.
