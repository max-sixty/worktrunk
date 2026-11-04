# DeepSeek Harness activity

`index.mjs` is a native Cordis plugin for local DSH profiles. It uses the existing `wt config state marker` interface; it adds no Worktrunk command, configuration key, or service. Mount it through a DSH invocation overlay as shown in the [agent integration guide](../../docs/src/content/docs/claude-code.md#deepseek-harness-dsh).

Only root agents contribute markers. Creation and resume mark working; real `agent/status` transitions mark working or waiting. Session working-directory changes move the contribution. Roots sharing a branch within one DSH process aggregate their state, with working taking precedence. Root disposal and plugin unload remove contributions; remounting observes existing roots.

Pending observations replace older observations for the same root. The worker therefore holds at most one active operation and one pending observation per root. A failed migration clear retains the old repository/branch anchor and withholds a new marker until a later observation can clear it. Cleanup uses the Git common directory, so the old session cwd need not exist. Disposing the plugin cancels and joins its active managed process range before clearing captured targets, serially within each repository.

The one-second process deadline requests cancellation; completion still depends on the subprocess provider confirming its managed range is empty. Shutdown work scales with the number of distinct branches. Missing or failing commands do not reject the agent's lifecycle.

Markers remain branch-scoped and have no owner token or conditional clear operation. Independent DSH processes and other integrations retain the existing last-writer-wins behavior. Cross-process aggregation would require a shared ownership protocol and is deliberately deferred for a maintainer decision.

## Development

The runtime module has no npm dependencies. This private test package pins the SDK and test-only model fixture dependencies separately:

```bash
cd dev/dsh
npm ci --ignore-scripts
WORKTRUNK_BIN=/absolute/path/to/wt npm test
```

The tests launch the installed DSH `sdk-minimal` profile through its public SDK, using disposable homes and Git worktrees. Only the model adapter is synthetic; Loader, agent lifecycle, persistence, subprocess management, Git, and successful Worktrunk commands are real. Failure cases substitute only the optional external marker command. No provider credentials or calls are used.

Validated with DSH 0.2.2-rc.1 and Worktrunk 0.81.0 on Linux. Windows, macOS, remote subprocess providers, and abrupt host termination remain unverified. The Windows executable selection follows the existing Pi integration, including the Windows Terminal alias guard.
