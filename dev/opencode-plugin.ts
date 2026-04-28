// Worktrunk activity tracking plugin for OpenCode.
//
// Tracks OpenCode session activity per branch, showing status markers in `wt list`:
//   🤖 — agent is working
//   💬 — agent is waiting for input
//
// Installed globally via: wt config plugins opencode install
// Or manually: copy to ~/.config/opencode/plugins/worktrunk.ts

import type { Plugin } from "@opencode-ai/plugin";

export default (async ({ $ }) => {
  return {
    event: async ({ event }) => {
      switch (event.type) {
        case "session.status":
          await $`wt config state marker set ${'🤖'} || true`.quiet();
          break;
        case "session.idle":
          await $`wt config state marker set ${'💬'} || true`.quiet();
          break;
        case "session.deleted":
          await $`wt config state marker clear || true`.quiet();
          break;
      }
    },
  };
}) satisfies Plugin;
