# Try it demo

The [Try it](https://worktrunk.dev/try-it/) page embeds a [Demoshell](https://demoshell.com) VM: Alpine Linux on an emulated arm64 CPU, running in the visitor's browser with `wt` installed. `docs/src/pages/try-it.astro` mounts it by its Demoshell reference.

The VM's definition is kept here and entered into the Demoshell studio, where the VM is built and saved:

| File | Studio field |
|------|--------------|
| `recipe.sh` | Base image script |
| `after-load.sh` | VM startup → A shell prompt → After VM load |
| `guide.json` | Guide: title, order, finish card, and steps |

`guide.json` uses the field names Demoshell's embed API serves for a guide. Each step ticks when the visitor runs `pattern`, and `output_contains` must appear in its output, so check those strings against real output when changing the recipe or `wt`'s messages.

To check the flow before a build, run `recipe.sh` without its `apk`, download, and `/etc/passwd` lines, using `HOME` set to a scratch directory and a local `wt` on `PATH`. Then, in a bash that sources the generated `.bash_profile`, run `after-load.sh` followed by each step's `pattern`.

Demoshell rebuilds the VM when a Worktrunk release is published, so the recipe installs the latest release rather than a pinned version.
