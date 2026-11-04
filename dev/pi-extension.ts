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

import { lstatSync } from "node:fs";
import { delimiter, join } from "node:path";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// The worktrunk binary, chosen as `plugins/worktrunk/hooks/wt.sh` chooses it.
// On Windows, `wt` is usually Windows Terminal's app-execution alias, which
// accepts any arguments, exits 0, and reports the unknown program in a popup
// window. So prefer `git-wt.exe` there, and never run a `wt` from WindowsApps.
// `lstat` rather than `exists`: the alias is a reparse point, and it has to be
// seen for the lookup to stop where Windows' own PATH search would.
// `undefined` means no worktrunk binary was found.
function worktrunkBin(): string | undefined {
  if (process.env.WORKTRUNK_BIN) return process.env.WORKTRUNK_BIN;
  if (process.platform !== "win32") return "wt";
  const dirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean);
  const find = (name: string) =>
    dirs
      .map((dir) => join(dir, name))
      .find((path) => {
        try {
          lstatSync(path);
          return true;
        } catch {
          return false;
        }
      });
  if (find("git-wt.exe")) return "git-wt.exe";
  const wt = find("wt.exe");
  return wt && !/[\\/]WindowsApps[\\/]/i.test(wt) ? "wt" : undefined;
}

export default function worktrunkActivity(pi: ExtensionAPI): void {
  const run = async (
    cwd: string,
    args: ["set", string] | ["clear"],
  ): Promise<void> => {
    const bin = worktrunkBin();
    if (!bin) return;
    try {
      await pi.exec(bin, ["config", "state", "marker", ...args], { cwd });
    } catch {
      // Activity tracking must never interrupt the host Pi session.
    }
  };

  pi.on("agent_start", async (_event, ctx) => {
    await run(ctx.cwd, ["set", "🤖"]);
  });

  // `agent_end` is not the end: Pi may auto-retry, auto-compact and retry, or
  // pick up a queued follow-up message. `agent_settled` is the event Pi
  // documents for status integrations — it fires once Pi will not continue
  // running on its own.
  pi.on("agent_settled", async (_event, ctx) => {
    await run(ctx.cwd, ["set", "💬"]);
  });

  pi.on("session_shutdown", async (_event, ctx) => {
    await run(ctx.cwd, ["clear"]);
  });
}
