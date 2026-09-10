// Worktrunk activity tracking extension for Pi (earendil-works/pi).
//
// Tracks agent activity per branch, showing status markers in `wt list`:
//   🤖 — agent is working
//   💬 — agent is waiting for input
//
// Installed globally via: wt config plugins pi install
//
// Pi loads user extensions from `~/.pi/agent/extensions/*.ts` (or
// `$PI_CODING_AGENT_DIR/extensions/`) and hands each factory an
// `ExtensionAPI`. oh-my-pi is a different agent with a different loader and
// API — see `dev/omp-hook.ts` and `wt config plugins omp install`.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function worktrunkActivity(pi: ExtensionAPI): void {
  const run = async (
    cwd: string,
    args: ["set", string] | ["clear"],
  ): Promise<void> => {
    try {
      await pi.exec("wt", ["config", "state", "marker", ...args], { cwd });
    } catch {
      // Activity tracking must never interrupt the host Pi session.
    }
  };

  pi.on("agent_start", async (_event, ctx) => {
    await run(ctx.cwd, ["set", "🤖"]);
  });

  pi.on("agent_end", async (_event, ctx) => {
    await run(ctx.cwd, ["set", "💬"]);
  });

  pi.on("session_shutdown", async (_event, ctx) => {
    await run(ctx.cwd, ["clear"]);
  });
}
