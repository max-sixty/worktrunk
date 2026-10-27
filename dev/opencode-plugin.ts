// Worktrunk plugin for OpenCode.
//
// Tracks OpenCode session activity per branch, showing status markers in `wt list`:
//   🤖 — agent is working
//   💬 — agent is waiting for input
//
// Under OpenCode 2 it also registers a worktree strategy, so worktrees OpenCode
// creates and removes go through `wt switch --create` and `wt remove`.
//
// Installed globally via: wt config plugins opencode install
// Or manually: copy to ~/.config/opencode/plugins/worktrunk.ts
//
// One file, two plugin runtimes. OpenCode 2 decodes the default export as
// `{ id, setup }` and loads nothing else — the bare default-exported function
// this file used to be is rejected without a message. OpenCode 1 reads
// `{ id, server }` and never looks at `setup`; 1.16 is the floor that matters,
// where the host began filtering events per plugin instance (the object shape
// itself loads as far back as 1.14.19). Both ignore the other's key, so
// a single installed file works either side of the version boundary.
//
// The types below are declared locally rather than imported from
// `@opencode-ai/plugin`: its `Plugin` type means different things in the two
// versions, and a lone file in the config directory has no package to resolve.

import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { basename, join } from "node:path";
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

type Signal = { signal: AbortSignal };

type WorktreeDefinition = {
  id: string;
  create(
    input: { sourceDirectory: string; directory: string; branch?: string },
    context: Signal,
  ): Promise<{ directory: string }>;
  remove(input: { directory: string; force: boolean }, context: Signal): Promise<void>;
  list(
    sourceDirectory: string,
    context: Signal,
  ): Promise<{ directory: string; type: "root" | "worktree" }[]>;
};

type SetupContext = {
  location?: { directory: string };
  event?: { subscribe(options?: { signal?: AbortSignal }): AsyncIterable<SessionEvent> };
  worktree?: {
    transform(callback: (editor: { add(definition: WorktreeDefinition): void }) => void): Promise<unknown>;
  };
};

/**
 * Run `wt` and return its stdout, surfacing its stderr as the error message.
 *
 * OpenCode reports a strategy's error to the user, and wt's own message (a
 * dirty worktree, a branch that already exists) says more than the exit code.
 */
async function wt(args: string[], { signal }: Signal): Promise<string> {
  try {
    const { stdout } = await execute("wt", args, { signal, maxBuffer: 16 * 1024 * 1024 });
    return stdout;
  } catch (error) {
    const stderr = (error as { stderr?: string }).stderr?.trim();
    throw new Error(stderr || (error instanceof Error ? error.message : String(error)));
  }
}

// OpenCode's own `git` strategy creates a detached checkout at a generated
// path. This one keeps worktrunk's model instead: each worktree gets a branch,
// named after the directory OpenCode suggests (the name the user typed, or a
// generated slug), at the path the `worktree-path` template picks. OpenCode
// accepts a strategy returning a different directory than it suggested.
//
// A dirty worktree fails removal with wt's message. OpenCode only offers to
// force when the error is its own `Worktree.OperationError` class, which a lone
// plugin file has no package to import from.
const strategy: WorktreeDefinition = {
  id: "worktrunk",
  async create({ sourceDirectory, directory, branch }, context) {
    const name = basename(directory);
    // `branch` is the starting ref, not the new branch's name. Reopen a branch
    // that already exists, as the Claude Code hook does, but refuse a starting
    // ref for one rather than silently ignore it.
    let exists = true;
    try {
      await execute("git", ["-C", sourceDirectory, "rev-parse", "--verify", "--quiet", `refs/heads/${name}`], context);
    } catch {
      exists = false;
    }
    if (exists && branch) {
      throw new Error(`Branch ${name} already exists, so it can't start from ${branch}`);
    }
    const args = ["-C", sourceDirectory, "switch", name, "--no-cd", "--format=json"];
    if (!exists) args.push("--create");
    if (branch) args.push("--base", branch);
    const { path } = JSON.parse(await wt(args, context)) as { path: string };
    return { directory: path };
  },
  async remove({ directory, force }, context) {
    // Already gone, whether removed by hand or by `wt remove` itself.
    if (!existsSync(join(directory, ".git"))) return;
    await wt(["-C", directory, "remove", "--foreground", ...(force ? ["--force"] : []), directory], context);
  },
  async list(sourceDirectory, context) {
    const { stdout } = await execute("git", ["-C", sourceDirectory, "worktree", "list", "--porcelain", "-z"], context);
    // Records are NUL-separated fields, each record ending in an empty field.
    // The first record is the main worktree, or the bare repository, which has
    // no checkout to report.
    const entries: { directory: string; type: "root" | "worktree" }[] = [];
    let fields: string[] = [];
    let first = true;
    for (const field of stdout.split("\0")) {
      if (field) {
        fields.push(field);
        continue;
      }
      if (fields.length === 0) continue;
      const path = fields[0].startsWith("worktree ") ? fields[0].slice("worktree ".length) : undefined;
      const skip = fields.some((f) => f === "bare" || f === "prunable" || f.startsWith("prunable "));
      if (path && !skip) entries.push({ directory: path, type: first ? "root" : "worktree" });
      first = false;
      fields = [];
    }
    return entries;
  },
};

async function setup(context: SetupContext) {
  // A host without worktree strategies passes no `worktree`, including the
  // OpenCode 1.18 loader noted below; activity tracking doesn't depend on it.
  await context.worktree?.transform((editor) => editor.add(strategy));

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

// --- OpenCode 1 (1.16+) -----------------------------------------------------

type ServerInput = { directory: string };
type HookInput = { event: { type: string } };

function server({ directory }: ServerInput) {
  // The host dispatches `event` without awaiting it, so a `set` can still be
  // running when the teardown below clears. Chaining every call onto one
  // promise is what puts the `clear` last; `disposed` drops the events the
  // host can still deliver, since it unsubscribes after calling `dispose`.
  let pending: Promise<void> = Promise.resolve();
  let disposed = false;
  const enqueue = (args: string[]): Promise<void> => {
    pending = pending.then(() => marker(directory, args));
    return pending;
  };

  return {
    // The host runs this when it tears the plugin instance down, which is what
    // a normal session exit does. `session.deleted` below only fires when a
    // session is explicitly deleted, so without `dispose` the last `set` stays
    // the final state and `wt list` shows a finished session as still active.
    // Part of the hook interface since 1.16, the floor the `server` path targets.
    dispose: async () => {
      disposed = true;
      await enqueue(["clear"]);
    },
    // OpenCode 1.16+ filters events to this plugin's directory before calling
    // the hook, so there is nothing to match on here. 1.15.x does not — it fans
    // every bus event to every plugin instance — so the marker there follows the
    // worktree the instance was created for, whichever session is active.
    event: async ({ event }: HookInput) => {
      if (disposed) return;
      switch (event.type) {
        case "session.status":
          await enqueue(["set", WORKING]);
          break;
        case "session.idle":
          await enqueue(["set", WAITING]);
          break;
        case "session.deleted":
          await enqueue(["clear"]);
          break;
      }
    },
  };
}

export default { id: "worktrunk", setup, server };
