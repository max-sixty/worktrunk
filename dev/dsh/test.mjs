import { test } from 'node:test';
import assert from 'node:assert/strict';
import { apply } from './index.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => Promise.withResolvers();
const root = (cwd = '/synthetic/checkout') => ({
  status: 'idle', session: { cwd, header: { origin: 'user' } },
});

function host(existing = [], spawn) {
  const events = new Map();
  const calls = [];
  const errors = [];
  let dispose;
  apply({
    agents: { list: () => existing },
    workingDirectory: { get: session => session.cwd },
    logger: { debug: error => errors.push(error) },
    subprocess: { spawn: spec => {
      // The pinned DSH API supports bounded collectors, not output 'ignore'.
      assert.deepEqual(spec.stdio, { stdin: 'ignore', stdout: { maxBytes: 1024 }, stderr: { maxBytes: 1024 } });
      calls.push(spec);
      return spawn?.(spec) ?? { done: Promise.resolve({ exitCode: 0 }), waitForExit: async () => {} };
    } },
    on: (event, handler) => events.set(event, handler),
    effect: setup => { dispose = setup(); },
  });
  return { calls, errors, dispose, emit: (event, agent, status) => events.get(event)({ agent, status }) };
}

test('root status and disposal use the initial checkout; subagents are ignored', async () => {
  const agent = root();
  const child = root();
  child.session.header.origin = 'subagent';
  const h = host();
  h.emit('agent/created', child);
  h.emit('agent/created', agent);
  await tick();
  agent.session.cwd = '/synthetic/changed';
  h.emit('agent/status', agent, 'running');
  await tick();
  h.emit('agent/status', agent, 'idle');
  await tick();
  await h.emit('agent/disposed', agent);
  assert.deepEqual(h.calls.map(c => c.argv.slice(4)), [['set', '💬'], ['set', '🤖'], ['set', '💬'], ['clear']]);
  assert.ok(h.calls.every(c => c.cwd === '/synthetic/checkout'));
  assert.deepEqual(h.calls[0].env, Object.fromEntries([
    'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_OBJECT_DIRECTORY',
  ].map(key => [key, undefined])));
  h.emit('agent/status', agent, 'running');
  await h.dispose();
  assert.equal(h.calls.length, 4);
});

for (const ending of ['agent', 'plugin']) {
  test(`${ending} disposal skips queued sets and joins the managed process before clear`, async () => {
    const agent = root();
    const direct = deferred();
    const managed = deferred();
    let first = true;
    const h = host([agent], () => {
      if (!first) return;
      first = false;
      return { done: direct.promise, waitForExit: () => managed.promise };
    });
    await tick();
    h.emit('agent/status', agent, 'running');
    h.emit('agent/status', agent, 'idle');
    const closed = ending === 'agent' ? h.emit('agent/disposed', agent) : h.dispose();
    direct.resolve({ exitCode: 0 });
    await tick();
    assert.equal(h.calls.length, 1, 'direct exit is not managed-range completion');
    managed.resolve();
    await closed;
    assert.deepEqual(h.calls.map(c => c.argv.slice(4)), [['set', '💬'], ['clear']]);
    await h.dispose();
  });
}

test('missing and failing optional commands do not break subsequent updates or teardown', async () => {
  const agent = root();
  let count = 0;
  const h = host([], () => {
    if (++count === 1) throw new Error('synthetic missing executable');
    return { done: Promise.resolve({ exitCode: 19 }), waitForExit: async () => {} };
  });
  h.emit('agent/created', agent);
  await tick();
  h.emit('agent/status', agent, 'running');
  await tick();
  await h.dispose();
  assert.equal(count, 3);
  assert.equal(h.errors.length, 1);
});

test('mount observes existing roots and preserves last-writer-wins behavior', async () => {
  const first = root();
  const second = root();
  second.status = 'running';
  const h = host([first, second]);
  await tick();
  await h.emit('agent/disposed', first);
  assert.deepEqual(h.calls.map(c => c.argv.slice(4)), [['set', '💬'], ['set', '🤖'], ['clear']]);
  await h.dispose();
});
