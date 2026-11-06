# Experimental DSH activity

Mount `index.mjs` with the overlay in the [agent integration guide](../../docs/src/content/docs/claude-code.md#experimental-deepseek-harness-dsh). The Worktrunk binary does not install this file.

Root agents mark their initial checkout as working or idle and clear on disposal or plugin unload. Keep that checkout and branch unchanged during the session. Directory/branch changes, worktree removal, and abrupt termination can leave a marker; clear it manually with `wt config state marker clear` in the affected checkout. Agents sharing a branch overwrite each other's markers, as with the other integrations.

The module uses Node built-ins and DSH's local subprocess provider. Run the dependency-free adapter tests with `node --test dev/dsh/test.mjs`. Tested with DSH 0.2.2-rc.1 and Worktrunk 0.81.0 on Linux; Windows, macOS, and remote providers remain unverified.
