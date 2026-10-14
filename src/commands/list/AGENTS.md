# List command

Render the local skeleton before starting forge requests. The skeleton uses local repository data; network results fill rows after first paint. In the picker, leaving cancels unfinished background requests. See the phase contract in src/commands/list/collect/mod.rs and the picker module docs.

When adding a column or preview, decide which data is needed for the skeleton and which can arrive later. Plain list is a run-to-completion command, so even progressive forge work delays exit; prefer picker-only live details where possible. A synchronous shell prompt must never touch the network; wt list statusline may fetch CI status because its host renders it asynchronously.

Measure first output with cargo bench --bench time_to_first_output list. Use wt-perf timeline for phase attribution; see benches/AGENTS.md.
