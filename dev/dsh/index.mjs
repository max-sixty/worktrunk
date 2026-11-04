/**
 * Worktrunk activity for local DSH profiles. Only root agents are tracked.
 *
 * Each root has at most one pending observation, replacing older events.
 * One managed operation is active; its captured targets are retained until
 * cancellation settles. A migration retains its old anchor until its update
 * succeeds. Disposal replaces pending updates with cleanup. Plugin unload
 * joins the active managed range, then clears captured targets serially.
 * Marker commands use the Git common directory, so a removed session cwd is
 * not required for cleanup. Aggregation is confined to this DSH process;
 * independent hosts retain Worktrunk's existing last-writer-wins semantics.
 */
import { lstatSync } from 'node:fs';
import { delimiter, join } from 'node:path';

export const name = 'worktrunk-activity';
export const inject = ['agents', 'workingDirectory', 'subprocess'];

// These are the same inherited Git path variables Worktrunk scrubs when it
// selects a worktree. The subprocess provider also removes credential vars.
const gitEnvironment = Object.fromEntries([
  'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_OBJECT_DIRECTORY',
].map(key => [key, undefined]));

// Request cancellation promptly; managed-range completion is provider-owned.
const processTimeoutMs = 1000;

// Match the existing Pi plugin's safe local Windows binary selection.
function worktrunkBin() {
  if (process.env.WORKTRUNK_BIN) return process.env.WORKTRUNK_BIN;
  if (process.platform !== 'win32') return 'wt';
  const directories = (process.env.PATH ?? '').split(delimiter).filter(Boolean);
  const find = name => directories.map(dir => join(dir, name)).find(path => {
    try { lstatSync(path); return true; } catch { return false; }
  });
  if (find('git-wt.exe')) return 'git-wt.exe';
  const wt = find('wt.exe');
  return wt && !/[\\/]WindowsApps[\\/]/i.test(wt) ? 'wt' : undefined;
}

/** Register lifecycle observations and own their asynchronous marker cleanup. */
export function apply(ctx) {
  const roots = new Map();
  const observations = new Map();
  let accepting = true;
  let draining;
  let active;

  async function command(argv, cwd, cancellation) {
    const signal = AbortSignal.any([cancellation, AbortSignal.timeout(processTimeoutMs)]);
    const handle = ctx.subprocess.spawn({
      argv, cwd, env: gitEnvironment, signal, graceMs: 100,
      stdio: { stdin: 'ignore', stdout: { maxBytes: 4096 }, stderr: { maxBytes: 1024 } },
    });
    let outcome;
    try {
      outcome = await handle.done;
    } finally {
      // The direct process can exit while a descendant still owns work.
      await handle.waitForExit();
    }
    if (signal.aborted || outcome.exitCode !== 0) return undefined;
    const output = handle.collected.stdout.readFrom(0);
    return output.lossy ? undefined : output.text.replace(/\r?\n$/, '');
  }

  function skipped(error) {
    // Activity is optional and must not reject initialization or teardown.
    ctx.logger.debug(`Worktrunk activity update skipped: ${String(error)}`);
  }

  async function targetAt(cwd, cancellation) {
    const ref = await command(['git', 'symbolic-ref', '--quiet', 'HEAD'], cwd, cancellation);
    if (!ref?.startsWith('refs/heads/')) return undefined;
    const branch = ref.slice('refs/heads/'.length);
    const gitDir = await command(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'], cwd, cancellation);
    return gitDir ? { gitDir, branch, key: JSON.stringify([gitDir, branch]) } : undefined;
  }

  async function refresh(target, cancellation, leaving) {
    if (!target) return true;
    const occupants = [...roots.values()].filter(root => root !== leaving && root.target?.key === target.key);
    const marker = occupants.some(root => root.marker === '🤖') ? '🤖' : occupants.length ? '💬' : undefined;
    const binary = worktrunkBin();
    if (!binary) return false;
    return (await command([
      binary, '-C', target.gitDir, 'config', 'state', 'marker',
      ...(marker === undefined ? ['clear'] : ['set', marker]), '--branch', target.branch,
    ], target.gitDir, cancellation)) !== undefined;
  }

  function uniqueTargets(targets) {
    return [...new Map(targets.filter(Boolean).map(target => [target.key, target])).values()];
  }

  async function drain() {
    while (observations.size) {
      const [agent, observation] = observations.entries().next().value;
      observations.delete(agent);
      const { root, cwd, revision } = observation;
      const operation = active = { agent, controller: new AbortController(), targets: [] };
      const cancellation = operation.controller.signal;
      try {
        if (!root.accepting) {
          roots.delete(agent);
          operation.targets = uniqueTargets([root.target, ...root.cleanupTargets]);
          for (const target of operation.targets) {
            try { await refresh(target, cancellation); } catch (error) { skipped(error); }
          }
          continue;
        }
        const next = await targetAt(cwd, cancellation);
        if (!accepting || !root.accepting || root.revision !== revision) continue;
        const previous = root.target;
        operation.targets = uniqueTargets([previous]);
        if (previous && previous.key !== next?.key) {
          // Failed clears keep one old anchor, rather than accumulating a
          // history of retired branches or forgetting a marker we wrote.
          if (!await refresh(previous, cancellation, root)) continue;
        }
        if (!accepting || !root.accepting || root.revision !== revision) continue;
        operation.targets = uniqueTargets([previous, next]);
        root.target = next;
        await refresh(next, cancellation);
      } catch (error) {
        skipped(error);
      } finally {
        active = undefined;
      }
    }
  }

  function schedule(agent, observation) {
    observations.set(agent, observation);
    if (draining) return;
    draining = drain().finally(() => {
      draining = undefined;
      if (observations.size) schedule(...observations.entries().next().value);
    });
  }

  function capture(agent, root, marker) {
    root.marker = marker;
    root.revision++;
    try {
      const cwd = ctx.workingDirectory.get(agent.session);
      schedule(agent, { root, cwd, revision: root.revision });
    } catch (error) {
      skipped(error);
    }
  }

  function track(agent, marker) {
    if (!accepting || roots.has(agent) || agent.session.header.origin === 'subagent' || ctx.agents.get(agent.id) !== agent) return;
    const root = { accepting: true, marker, revision: 0, target: undefined, cleanupTargets: [] };
    roots.set(agent, root);
    capture(agent, root, marker);
  }

  ctx.on('agent/created', ({ agent }) => { track(agent, '🤖'); });

  ctx.on('agent/status', ({ agent, status }) => {
    const root = roots.get(agent);
    if (accepting && root?.accepting) capture(agent, root, status === 'running' ? '🤖' : '💬');
  });

  ctx.on('session/event', (session, event) => {
    if (!accepting || event.type !== 'working-directory/change') return;
    for (const [agent, root] of roots) {
      if (root.accepting && agent.session === session) capture(agent, root, root.marker);
    }
  });

  ctx.on('agent/disposed', ({ agent }) => {
    const root = roots.get(agent);
    if (!root?.accepting) return;
    root.accepting = false;
    if (active?.agent === agent) {
      root.cleanupTargets = uniqueTargets([...root.cleanupTargets, ...active.targets]);
      active.controller.abort();
    }
    schedule(agent, { root });
  });

  ctx.effect(() => async () => {
    accepting = false;
    observations.clear();
    const targets = uniqueTargets([
      ...[...roots.values()].flatMap(root => [root.target, ...root.cleanupTargets]),
      ...(active?.targets ?? []),
    ]);
    roots.clear();
    active?.controller.abort();
    await draining;
    const cancellation = new AbortController().signal;
    for (const target of targets) {
      try { await refresh(target, cancellation); } catch (error) { skipped(error); }
    }
  });

  // A Loader remount must also observe roots that already exist.
  for (const agent of ctx.agents.list()) track(agent, agent.status === 'running' ? '🤖' : '💬');
}
