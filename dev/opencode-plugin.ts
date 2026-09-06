// Worktrunk activity tracking plugin for OpenCode.
//
// Tracks OpenCode session activity per branch, showing status markers in `wt list`:
//   🤖 — agent is working
//   💬 — agent is waiting for input
//
// Installed globally via: wt config plugins opencode install
// Or manually: copy to ~/.config/opencode/plugins/worktrunk.ts
//
// One file, two plugin runtimes. OpenCode 2 decodes the default export as
// `{ id, setup }` and loads nothing else — the bare default-exported function
// this file used to be is rejected without a message. OpenCode 1 (1.17+) reads
// `{ id, server }` and never looks at `setup`. Both ignore the other's key, so
// a single installed file works either side of the version boundary.
//
// The types below are declared locally rather than imported from
// `@opencode-ai/plugin`: its `Plugin` type means different things in the two
// versions, and a lone file in the config directory has no package to resolve.

import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execute = promisify(execFile);

const WORKING = "🤖";
const WAITING = "💬";

/**
 * Run `wt config state marker …` in this plugin instance's worktree.
 *
 * `cwd` is load-bearing: the marker belongs to the worktree the session is in,
 * not to whatever directory the host process happens to sit in.
 */
async function marker(directory: string, args: string[]): Promise<void> {
  try {
    await execute("wt", ["config", "state", "marker", ...args], { cwd: directory });
  } catch {
    // `wt` may be off PATH, or the directory may no longer be a worktree. An
    // activity marker is not worth raising an error into the session over.
  }
}

// --- OpenCode 2 -------------------------------------------------------------

type SessionEvent = {
  type: string;
  location?: { directory: string };
  data?: { status?: { type?: string } };
};

type SetupContext = {
  location?: { directory: string };
  event?: { subscribe(options?: { signal?: AbortSignal }): AsyncIterable<SessionEvent> };
};

function setup(context: SetupContext) {
  const directory = context.location?.directory;
  const events = context.event;
  // OpenCode 1.18 also calls `setup`, from a second loader that passes neither
  // of these. Its `server` hook below drives the marker there, so bail quietly
  // rather than throwing out of a plugin the host did load.
  if (!directory || !events) return;

  const controller = new AbortController();

  const watcher = (async () => {
    for await (const event of events.subscribe({ signal: controller.signal })) {
      // The event envelope makes `location` optional; the session events below
      // carry one, and skipping the rest keeps each instance to its own worktree.
      if (event.location?.directory !== directory) continue;

      switch (event.type) {
        case "session.status":
          // Status is `idle | busy | retry`. OpenCode 2 marks `session.idle`
          // deprecated in favor of this, so read the status, not the event name.
          await marker(directory, ["set", event.data?.status?.type === "idle" ? WAITING : WORKING]);
          break;
        case "session.idle":
          await marker(directory, ["set", WAITING]);
          break;
        case "session.deleted":
          await marker(directory, ["clear"]);
          break;
      }
    }
  })().catch((error: unknown) => {
    if (!controller.signal.aborted) {
      console.error("Worktrunk activity tracking stopped", error);
    }
  });

  // Awaiting the watcher before clearing keeps an in-flight `set` from landing
  // after the `clear` and leaving a marker behind.
  return async () => {
    controller.abort();
    await watcher;
    await marker(directory, ["clear"]);
  };
}

// --- OpenCode 1 (1.17+) -----------------------------------------------------

type ServerInput = { directory: string };
type HookInput = { event: { type: string } };

function server({ directory }: ServerInput) {
  return {
    // OpenCode 1 filters events to this plugin's directory before calling the
    // hook, so there is nothing to match on here.
    event: async ({ event }: HookInput) => {
      switch (event.type) {
        case "session.status":
          await marker(directory, ["set", WORKING]);
          break;
        case "session.idle":
          await marker(directory, ["set", WAITING]);
          break;
        case "session.deleted":
          await marker(directory, ["clear"]);
          break;
      }
    },
  };
}

export default { id: "worktrunk", setup, server };
