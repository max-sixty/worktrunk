import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { access, mkdir, readFile, writeFile, rename, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir, devNull } from 'node:os';
import { DeepSeekHarness } from '@deepseek-ai/dsh-sdk-client';

const execute = promisify(execFile);
const wt = process.env.WORKTRUNK_BIN ?? 'wt';
const worlds = await mkdtemp(join(tmpdir(), 'worktrunk-dsh-'));
after(() => rm(worlds, { recursive: true, force: true }));
const gitEnv = {
  PATH: process.env.PATH, LANG: 'C.UTF-8',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull,
};
const checkpoints = [];

async function git(cwd, ...args) {
  return (await execute('git', ['-C', cwd, ...args], { env: gitEnv })).stdout.trim();
}

async function marker(world, branch) {
  try {
    return JSON.parse(await git(world.main, 'config', '--get', `worktrunk.state.${branch}.marker`)).marker;
  } catch (error) {
    if (error.code === 1) return undefined;
    throw error;
  }
}

async function until(read, expected, label, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  let actual;
  do {
    actual = await read();
    if (actual === expected) return;
    await new Promise(resolve => setTimeout(resolve, 20));
  } while (Date.now() < deadline);
  assert.equal(actual, expected, label);
}

async function requested(world, id) {
  await until(async () => {
    try { await access(join(world.gates, `${id}.requested.json`)); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, `real model boundary ${id}`);
}

async function release(world, id) {
  await writeFile(join(world.gates, `${id}.release`), 'continue\n');
}

async function checkpoint(world, stage) {
  const row = { world: world.root, stage,
    alpha: await marker(world, 'feature/alpha'), beta: await marker(world, 'feature/beta') };
  checkpoints.push(row);
  console.log(JSON.stringify(row));
}

async function world(name, control = false) {
  const root = join(worlds, name);
  const main = join(root, 'main checkout');
  const alpha = join(root, 'alpha checkout');
  const beta = join(root, 'beta checkout');
  const gates = join(root, 'model gates');
  const controls = join(root, 'lifecycle control');
  await mkdir(main, { recursive: true });
  await mkdir(gates, { recursive: true });
  await mkdir(controls, { recursive: true });
  await git(main, 'init', '-b', 'main');
  await writeFile(join(main, 'README'), 'synthetic integration data\n');
  await git(main, 'add', 'README');
  await git(main, '-c', 'user.name=Synthetic Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'synthetic fixture');
  await git(main, 'worktree', 'add', '-b', 'feature/alpha', alpha);
  await git(main, 'worktree', 'add', '-b', 'feature/beta', beta);
  // Git's abbreviated symbolic ref is ambiguous when a tag has this name.
  await git(main, 'tag', 'feature/alpha');
  const overlay = join(root, 'activity.patch.yml');
  await writeFile(overlay, [
    '- insert:',
    '    - id: worktrunk-activity',
    `      name: ${JSON.stringify(new URL('../index.mjs', import.meta.url).href)}`,
    '    - id: worktrunk-gated-llm-fixture',
    `      name: ${JSON.stringify(new URL('./gated-llm.mjs', import.meta.url).href)}`,
    ...(control ? [
      '    - id: worktrunk-lifecycle-control-fixture',
      `      name: ${JSON.stringify(new URL('./lifecycle-control.mjs', import.meta.url).href)}`,
    ] : []),
    '',
  ].join('\n'));
  return { root, main, alpha, beta, gates, overlay, controls };
}

function client(world, tag, extra = {}) {
  return new DeepSeekHarness({
    cwd: world.alpha, processCwd: world.root,
    profile: 'sdk-minimal', dshHome: join(world.root, 'dsh-home'),
    patches: [world.overlay], provider: 'worktrunk-fixture', model: 'synthetic-model',
    initializeTimeoutMs: 30000, requestTimeoutMs: 30000,
    env: {
      PATH: process.env.PATH,
      ...(process.env.WORKTRUNK_BIN ? { WORKTRUNK_BIN: process.env.WORKTRUNK_BIN } : {}),
      HOME: join(world.root, 'home'),
      XDG_CONFIG_HOME: join(world.root, 'xdg-config'), XDG_DATA_HOME: join(world.root, 'xdg-data'),
      LANG: 'C.UTF-8', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull,
      GIT_DIR: join(world.main, '.git'), GIT_WORK_TREE: world.main,
      DSH_AGENTS_HOME: join(world.root, 'agents'), DSH_PRIMARY_RUNTIME: '', DSH_TELEMETRY_DISABLED: '1',
      WT_DSH_GATE_DIR: world.gates, WT_DSH_GATE_TAG: tag,
      WT_DSH_CONTROL_DIR: world.controls,
      ...extra,
    },
  });
}

test('installed DSH roots mark two real worktrees, settle, resume across processes and clear', { timeout: 90000 }, async t => {
  const state = await world(`lifecycle-${Date.now()}`);
  const first = client(state, 'first');
  t.after(() => first.close());
  const alpha = first.session('alpha');
  const beta = first.session('beta');
  assert.equal(await alpha.getWorkingDirectory(), state.alpha);
  await until(() => marker(state, 'feature/alpha'), '🤖', 'fresh root start');
  assert.equal(await beta.setWorkingDirectory(state.beta), state.beta);
  await until(() => marker(state, 'feature/beta'), '🤖', 'second root cwd');
  assert.equal(await marker(state, 'feature/alpha'), '🤖');
  await checkpoint(state, 'two-root-start');

  let run = alpha.run('synthetic alpha prompt');
  await requested(state, 'first-1');
  await until(() => marker(state, 'feature/alpha'), '🤖', 'running alpha');
  await release(state, 'first-1');
  assert.match((await run).finalResponse, /Synthetic reply/);
  await until(() => marker(state, 'feature/alpha'), '💬', 'settled alpha');
  assert.equal(await marker(state, 'feature/beta'), '🤖');
  await checkpoint(state, 'alpha-settled-beta-unchanged');

  run = beta.run('synthetic beta prompt');
  await requested(state, 'first-2');
  await until(() => marker(state, 'feature/beta'), '🤖', 'running beta');
  await release(state, 'first-2');
  await run;
  await until(() => marker(state, 'feature/beta'), '💬', 'settled beta');

  assert.equal(await alpha.setWorkingDirectory(state.beta), state.beta);
  await until(() => marker(state, 'feature/alpha'), undefined, 'old worktree cleared after migration');
  assert.equal(await marker(state, 'feature/beta'), '💬');
  run = alpha.run('synthetic alpha prompt after moving to beta');
  await requested(state, 'first-3');
  await until(() => marker(state, 'feature/beta'), '🤖', 'working root wins over idle occupant');
  assert.equal(await marker(state, 'feature/alpha'), undefined);
  await checkpoint(state, 'both-roots-in-beta-one-working');
  await release(state, 'first-3');
  await run;
  await until(() => marker(state, 'feature/beta'), '💬', 'shared branch settled');
  assert.equal(await alpha.setWorkingDirectory(state.alpha), state.alpha);
  await until(() => marker(state, 'feature/alpha'), '💬', 'return to alpha');
  assert.equal(await marker(state, 'feature/beta'), '💬');

  run = alpha.run('second synthetic alpha prompt');
  await requested(state, 'first-4');
  await until(() => marker(state, 'feature/alpha'), '🤖', 'alpha reawakened');
  await checkpoint(state, 'alpha-reawakened-beta-idle');
  await release(state, 'first-4');
  await run;
  await until(() => marker(state, 'feature/alpha'), '💬', 'alpha settled twice');

  const listing = (await execute(wt, ['-C', state.alpha, 'list'], { env: gitEnv })).stdout;
  assert.ok(listing.includes('feature/alpha') && listing.includes('feature/beta') && listing.includes('💬'));
  await writeFile(join(state.root, 'wt-list.txt'), listing);
  await first.close();
  await until(() => marker(state, 'feature/alpha'), undefined, 'alpha disposed');
  await until(() => marker(state, 'feature/beta'), undefined, 'beta disposed');
  await checkpoint(state, 'first-host-disposed');

  const second = client(state, 'resumed');
  t.after(() => second.close());
  const resumedAlpha = await second.resumeSession('alpha');
  await until(() => marker(state, 'feature/alpha'), '🤖', 'persisted alpha resume');
  const resumedBeta = await second.resumeSession('beta');
  assert.equal(await resumedBeta.getWorkingDirectory(), state.beta);
  await until(() => marker(state, 'feature/beta'), '🤖', 'persisted beta cwd');
  await checkpoint(state, 'second-host-resumed');
  run = resumedAlpha.run('synthetic persisted followup');
  await requested(state, 'resumed-1');
  await release(state, 'resumed-1');
  await run;
  await until(() => marker(state, 'feature/alpha'), '💬', 'resumed alpha settled');
  await second.close();
  await until(() => marker(state, 'feature/alpha'), undefined, 'resumed alpha disposed');
  await until(() => marker(state, 'feature/beta'), undefined, 'resumed beta disposed');
  await checkpoint(state, 'second-host-disposed');
  assert.equal(await git(state.main, 'status', '--porcelain'), '');
});

test('missing real wt executable does not block the installed DSH host', { timeout: 60000 }, async t => {
  const state = await world(`absent-${Date.now()}`);
  const host = client(state, 'absent', { WORKTRUNK_BIN: join(state.root, 'not-installed-wt') });
  t.after(() => host.close());
  const session = host.session('missing-wt');
  const started = performance.now();
  assert.equal(await session.getWorkingDirectory(), state.alpha);
  const creationMs = performance.now() - started;
  assert.ok(creationMs < 5000, `missing wt creation took ${creationMs} ms`);
  const run = session.run('synthetic prompt with wt absent');
  await requested(state, 'absent-1');
  await release(state, 'absent-1');
  assert.match((await run).finalResponse, /Synthetic reply/);
  await host.close();
  assert.equal(await marker(state, 'feature/alpha'), undefined);
  checkpoints.push({ stage: 'missing-wt-completed', creationMs, root: state.root });
  console.log(JSON.stringify(checkpoints.at(-1)));
});

test('nonzero optional marker command cannot reject a real root lifecycle', { timeout: 60000 }, async t => {
  const state = await world(`nonzero-${Date.now()}`);
  const failing = join(state.root, 'failing marker command');
  await writeFile(failing, '#!/bin/sh\nexit 19\n', { mode: 0o755 });
  const host = client(state, 'nonzero', { WORKTRUNK_BIN: failing });
  t.after(() => host.close());
  const session = host.session('nonzero-wt');
  assert.equal(await session.getWorkingDirectory(), state.alpha);
  const run = session.run('synthetic prompt with marker failure');
  await requested(state, 'nonzero-1');
  await release(state, 'nonzero-1');
  assert.match((await run).finalResponse, /Synthetic reply/);
  await host.close();
  assert.equal(await marker(state, 'feature/alpha'), undefined);
});

test('a hung optional command is terminated and drained by the real subprocess provider', { timeout: 60000 }, async t => {
  const state = await world(`timeout-${Date.now()}`);
  const hanging = join(state.root, 'hanging marker command');
  const pids = join(state.root, 'marker-processes.txt');
  await writeFile(hanging, `#!${process.execPath}\n` +
    `import {appendFileSync} from 'node:fs';\n` +
    `appendFileSync(${JSON.stringify(pids)}, String(process.pid) + '\\n');\n` +
    `process.on('SIGTERM', () => {});\nsetInterval(() => {}, 1000);\n`, { mode: 0o755 });
  const host = client(state, 'timeout', { WORKTRUNK_BIN: hanging });
  t.after(() => host.close());
  const started = performance.now();
  const session = host.session('slow-wt');
  assert.equal(await session.getWorkingDirectory(), state.alpha);
  assert.ok(performance.now() - started < 5000, 'one timed-out marker must not hang creation');
  await until(async () => {
    try { await access(pids); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, 'the hung optional command actually started');
  const run = session.run('synthetic prompt with a hung marker command');
  await requested(state, 'timeout-1');
  await release(state, 'timeout-1');
  assert.match((await run).finalResponse, /Synthetic reply/);
  await host.close();
  const ownedPids = (await readFile(pids, 'utf8')).trim().split('\n').map(Number);
  assert.ok(ownedPids.length >= 1, 'the failure command must actually run');
  for (const pid of ownedPids) {
    assert.throws(() => process.kill(pid, 0), error => error.code === 'ESRCH', `marker child ${pid} remained live`);
  }
  assert.equal(await marker(state, 'feature/alpha'), undefined);
  console.log(JSON.stringify({ stage: 'hung-marker-processes-reaped', count: ownedPids.length,
    lifecycleMs: performance.now() - started, root: state.root }));
});

test('disposal clears through the Git common directory after the session cwd disappears', { timeout: 45000 }, async t => {
  const state = await world(`missing-cwd-${Date.now()}`);
  const host = client(state, 'missing-cwd');
  t.after(() => host.close());
  assert.equal(await host.session('missing-cwd').getWorkingDirectory(), state.alpha);
  await until(() => marker(state, 'feature/alpha'), '🤖', 'marker exists before cwd is moved');
  const moved = `${state.alpha}.retained`;
  await rename(state.alpha, moved);
  t.after(() => rename(moved, state.alpha));
  await host.close();
  await until(() => marker(state, 'feature/alpha'), undefined, 'clear must not require the old cwd', 1500);
});

test('rapid real turns coalesce marker work instead of accumulating a shutdown backlog', { timeout: 45000 }, async t => {
  const state = await world(`coalescing-${Date.now()}`);
  const hanging = join(state.root, 'slow marker command');
  const pids = join(state.root, 'slow-processes.txt');
  await writeFile(hanging, `#!${process.execPath}\n` +
    `import {appendFileSync} from 'node:fs';\n` +
    `appendFileSync(${JSON.stringify(pids)}, String(process.pid) + '\\n');\n` +
    `process.on('SIGTERM', () => {});\nsetInterval(() => {}, 1000);\n`, { mode: 0o755 });
  const host = client(state, 'coalescing', { WORKTRUNK_BIN: hanging, WT_DSH_GATE_AUTO: '1' });
  t.after(() => host.close());
  const session = host.session('coalescing');
  await session.getWorkingDirectory();
  await until(async () => {
    try { await access(pids); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, 'the slow command actually started');
  for (let turn = 0; turn < 15; turn++) {
    assert.match((await session.run(`synthetic rapid turn ${turn}`)).finalResponse, /Synthetic reply/);
  }
  const closing = performance.now();
  await host.close();
  const closeMs = performance.now() - closing;
  assert.ok(closeMs < 4000, `marker backlog delayed shutdown by ${closeMs} ms`);
  const ownedPids = (await readFile(pids, 'utf8')).trim().split('\n').map(Number);
  for (const pid of ownedPids) assert.throws(() => process.kill(pid, 0), error => error.code === 'ESRCH');
  console.log(JSON.stringify({ stage: 'bounded-marker-backlog', closeMs, processes: ownedPids.length }));
});

let controlSequence = 0;
async function control(state, input) {
  const id = String(++controlSequence).padStart(4, '0');
  const pending = join(state.controls, `${id}.pending`);
  await writeFile(pending, JSON.stringify(input));
  await rename(pending, join(state.controls, `${id}.command.json`));
  const result = join(state.controls, `${id}.result.json`);
  await until(async () => {
    try { await access(result); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, `real lifecycle control ${input.action}`, 20000);
  const value = JSON.parse(await readFile(result, 'utf8'));
  assert.equal(value.ok, true, value.error);
  return value.result;
}

async function remains(read, expected, durationMs = 1500) {
  const end = Date.now() + durationMs;
  do {
    assert.equal(await read(), expected);
    await new Promise(resolve => setTimeout(resolve, 20));
  } while (Date.now() < end);
}

test('real root disposal, subagent exclusion and Loader remount preserve live ownership', { timeout: 60000 }, async t => {
  const state = await world(`ownership-${Date.now()}`, true);
  const host = client(state, 'ownership');
  t.after(() => host.close());
  await host.start();
  await control(state, { action: 'create', id: 'first-root', cwd: state.alpha });
  await until(() => marker(state, 'feature/alpha'), '🤖', 'real first root');
  const child = await control(state, { action: 'create', id: 'actual-child', cwd: state.beta, subagent: true, parent: 'first-root' });
  assert.ok(child.agents.some(agent => agent.id === 'actual-child' && agent.origin === 'subagent'));
  await remains(() => marker(state, 'feature/beta'), undefined);
  await control(state, { action: 'create', id: 'second-root', cwd: state.alpha });
  await control(state, { action: 'dispose', id: 'first-root' });
  await remains(() => marker(state, 'feature/alpha'), '🤖');
  await control(state, { action: 'reload' });
  await until(() => marker(state, 'feature/alpha'), '💬', 'existing idle root observed on real remount');
  const turn = control(state, { action: 'turn', id: 'second-root', text: 'synthetic prompt after reload' });
  await requested(state, 'ownership-1');
  await until(() => marker(state, 'feature/alpha'), '🤖', 'remounted plugin sees real running state');
  await release(state, 'ownership-1');
  await turn;
  await until(() => marker(state, 'feature/alpha'), '💬', 'remounted root settled');
  await control(state, { action: 'dispose', id: 'second-root' });
  await until(() => marker(state, 'feature/alpha'), undefined, 'last real root disposed');
  await control(state, { action: 'dispose', id: 'actual-child' });
  await host.close();
});

test('a failed migration clear remains reachable for later real disposal', { timeout: 45000 }, async t => {
  const state = await world(`failed-clear-${Date.now()}`);
  const wrapper = join(state.root, 'fail first clear');
  const failed = join(state.root, 'clear-failed-once');
  await writeFile(wrapper, `#!${process.execPath}\n` +
    `import {existsSync,writeFileSync} from 'node:fs';\nimport {spawnSync} from 'node:child_process';\n` +
    `if(process.argv.includes('clear')&&!existsSync(${JSON.stringify(failed)})){writeFileSync(${JSON.stringify(failed)},'failed');process.exit(19);}\n` +
    `const result=spawnSync(${JSON.stringify(wt)},process.argv.slice(2),{stdio:'inherit'});process.exit(result.status??1);\n`, { mode: 0o755 });
  const host = client(state, 'failed-clear', { WORKTRUNK_BIN: wrapper, WT_DSH_GATE_AUTO: '1' });
  t.after(() => host.close());
  const session = host.session('moved-root');
  await session.getWorkingDirectory();
  await until(() => marker(state, 'feature/alpha'), '🤖', 'old marker before failed transition');
  await session.setWorkingDirectory(state.beta);
  await until(async () => {
    try { await access(failed); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, 'actual old-branch clear failed');
  await session.run('synthetic turn after the failed migration');
  await until(() => marker(state, 'feature/beta'), '💬', 'a later real idle observation completed the migration');
  await host.close();
  await until(() => marker(state, 'feature/alpha'), undefined, 'old branch remains available for cleanup', 1500);
  assert.equal(await marker(state, 'feature/beta'), undefined);
});

test('real disposal joins a delayed descendant before the final clear', { timeout: 45000 }, async t => {
  const state = await world(`descendant-${Date.now()}`, true);
  const wrapper = join(state.root, 'descendant marker command');
  const child = join(state.root, 'delayed-child.mjs');
  const ready = join(state.root, 'child-ready');
  const spawned = join(state.root, 'child-spawned');
  const fired = join(state.root, 'late-write-fired');
  await writeFile(child, `import {writeFileSync} from 'node:fs';\nimport {spawnSync} from 'node:child_process';\n` +
    `process.on('SIGTERM',()=>{});writeFileSync(${JSON.stringify(ready)},String(process.pid));\n` +
    `setTimeout(()=>{spawnSync(${JSON.stringify(wt)},process.argv.slice(2),{stdio:'ignore'});writeFileSync(${JSON.stringify(fired)},'fired');},600);\n`);
  await writeFile(wrapper, `#!${process.execPath}\n` +
    `import {existsSync,writeFileSync} from 'node:fs';\nimport {spawn,spawnSync} from 'node:child_process';\n` +
    `const result=spawnSync(${JSON.stringify(wt)},process.argv.slice(2),{stdio:'inherit'});\n` +
    `if(process.argv.includes('set')&&!existsSync(${JSON.stringify(spawned)})){writeFileSync(${JSON.stringify(spawned)},'yes');const child=spawn(process.execPath,[${JSON.stringify(child)},...process.argv.slice(2)],{stdio:'ignore'});child.unref();}\n` +
    `process.exit(result.status??1);\n`, { mode: 0o755 });
  const host = client(state, 'descendant', { WORKTRUNK_BIN: wrapper });
  t.after(() => host.close());
  await host.start();
  await control(state, { action: 'create', id: 'descendant-root', cwd: state.alpha });
  await until(() => marker(state, 'feature/alpha'), '🤖', 'real first marker write');
  await until(async () => {
    try { await access(ready); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, true, 'the delayed child is genuinely alive');
  await control(state, { action: 'dispose', id: 'descendant-root' });
  await until(() => marker(state, 'feature/alpha'), undefined, 'real final clear');
  await remains(() => marker(state, 'feature/alpha'), undefined);
  await assert.rejects(access(fired), error => error.code === 'ENOENT');
  await host.close();
});
