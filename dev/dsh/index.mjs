// Experimental activity markers for local DSH profiles with a fixed checkout.
// Like the other integrations, markers are branch-scoped and last-writer-wins.
import { lstatSync } from 'node:fs';
import { delimiter, join } from 'node:path';

export const name = 'worktrunk-activity';
export const inject = ['agents', 'workingDirectory', 'subprocess'];

const gitEnvironment = Object.fromEntries([
  'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_OBJECT_DIRECTORY',
].map(key => [key, undefined]));

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

export function apply(ctx) {
  const roots = new Map();
  let closing = false;
  let pending = Promise.resolve();

  function run(agent, cwd, marker) {
    // DSH status/disposal events do not await their listeners. Keep writes in
    // event order and skip queued sets after their root or this plugin closes.
    pending = pending.then(async () => {
      if (marker && (closing || !roots.has(agent))) return;
      const binary = worktrunkBin();
      if (!binary) return;
      const handle = ctx.subprocess.spawn({
        argv: [binary, 'config', 'state', 'marker', ...(marker ? ['set', marker] : ['clear'])],
        cwd, env: gitEnvironment, signal: AbortSignal.timeout(1000), graceMs: 100,
        stdio: { stdin: 'ignore', stdout: { maxBytes: 1024 }, stderr: { maxBytes: 1024 } },
      });
      try { await handle.done; }
      finally { await handle.waitForExit(); }
    }).catch(error => ctx.logger.debug(`Worktrunk activity update skipped: ${String(error)}`));
    return pending;
  }

  function track(agent) {
    if (closing || roots.has(agent) || agent.session.header.origin === 'subagent') return;
    try {
      const cwd = ctx.workingDirectory.get(agent.session);
      roots.set(agent, cwd);
      run(agent, cwd, agent.status === 'running' ? '🤖' : '💬');
    } catch (error) {
      ctx.logger.debug(`Worktrunk activity update skipped: ${String(error)}`);
    }
  }

  ctx.on('agent/created', ({ agent }) => { track(agent); });
  ctx.on('agent/status', ({ agent, status }) => {
    if (!closing && roots.has(agent)) run(agent, roots.get(agent), status === 'running' ? '🤖' : '💬');
  });
  ctx.on('agent/disposed', ({ agent }) => {
    if (!roots.has(agent)) return;
    const cwd = roots.get(agent);
    roots.delete(agent);
    return run(agent, cwd);
  });
  ctx.effect(() => () => {
    closing = true;
    for (const [agent, cwd] of roots) run(agent, cwd);
    roots.clear();
    return pending;
  });
  for (const agent of ctx.agents.list()) track(agent);
}
