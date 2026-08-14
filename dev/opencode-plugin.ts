// Worktrunk activity tracking plugin for OpenCode.
//
// Tracks OpenCode session activity per branch, showing status markers in `wt list`:
//   🤖 — agent is working
//   💬 — agent is waiting for input
//
// Installed globally via: wt config plugins opencode install
// Or manually: copy to ~/.config/opencode/plugins/worktrunk.ts

import type { Plugin } from "@opencode-ai/plugin";

export default (async ({ $, directory }) => {
  return {
    event: async ({ event }) => {
      // `$` is the host's process-global Bun shell, shared by every plugin in
      // the process. The promise-level `.cwd(directory)` on each command below
      // is load-bearing: it pins the command to the directory this plugin
      // instance was created for. Without it, the command runs in whatever the
      // process-wide cwd happens to be at spawn time. Do not "simplify" to the
      // instance-level `$.cwd(...)` — that mutates the shared default for every
      // plugin in the host process.
      switch (event.type) {
        case "session.status":
          await $`wt config state marker set ${'🤖'} || true`.cwd(directory).quiet();
          break;
        case "session.idle":
          await $`wt config state marker set ${'💬'} || true`.cwd(directory).quiet();
          break;
        case "session.deleted":
          await $`wt config state marker clear || true`.cwd(directory).quiet();
          break;
      }
    },
  };
}) satisfies Plugin;
