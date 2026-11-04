// Test-only control of real AgentHandles and the actual Loader Entry.
import { readdir, readFile, writeFile, unlink, rename } from 'node:fs/promises';
import { join } from 'node:path';
import { SessionId } from '@deepseek-ai/dsh-session';
import { createUserMessage } from '@deepseek-ai/dsh-llm';

export const name = 'worktrunk-lifecycle-control-fixture';
export const inject = ['agents', 'workingDirectory', 'loader'];

export function apply(ctx) {
  const handles = new Map();
  const directory = process.env.WT_DSH_CONTROL_DIR;
  let working = false;
  let current = Promise.resolve();

  async function command(input) {
    if (input.action === 'create') {
      const handle = await ctx.agents.create({
        sessionId: SessionId(input.id),
        meta: { cwd: input.cwd, ...(input.subagent ? { origin: 'subagent', parentSession: SessionId(input.parent) } : {}) },
        agentOptions: { provider: 'worktrunk-fixture', model: 'synthetic-model' },
      });
      handles.set(input.id, handle);
    } else if (input.action === 'dispose') {
      await handles.get(input.id).dispose();
      handles.delete(input.id);
    } else if (input.action === 'turn') {
      const agent = handles.get(input.id).agent;
      agent.followup(createUserMessage({ content: [{ type: 'text', text: input.text }], source: { kind: 'user' } }));
      await agent.whenIdle();
    } else if (input.action === 'reload') {
      const entry = [...ctx.loader.entries()].find(entry => entry.options.id === 'worktrunk-activity');
      if (!entry) throw new Error('actual activity Loader Entry missing');
      await entry.update({ disabled: true });
      await entry.update({ disabled: false });
      await ctx.loader.await();
    } else {
      throw new Error(`Unknown test action ${input.action}`);
    }
    return { agents: ctx.agents.list().map(agent => ({ id: agent.id, origin: agent.session.header.origin })) };
  }

  async function scan() {
    for (const file of (await readdir(directory)).filter(file => file.endsWith('.command.json')).sort()) {
      const input = JSON.parse(await readFile(join(directory, file), 'utf8'));
      let result;
      try { result = { ok: true, result: await command(input) }; }
      catch (error) { result = { ok: false, error: String(error) }; }
      await unlink(join(directory, file));
      const pending = join(directory, file.replace('.command.json', '.result.pending'));
      await writeFile(pending, JSON.stringify(result));
      await rename(pending, join(directory, file.replace('.command.json', '.result.json')));
    }
  }

  ctx.effect(() => {
    const timer = setInterval(() => {
      if (working) return;
      working = true;
      current = scan().finally(() => { working = false; });
    }, 10);
    return async () => {
      clearInterval(timer);
      await current;
      await Promise.all([...handles.values()].map(handle => handle.dispose()));
    };
  });
}
