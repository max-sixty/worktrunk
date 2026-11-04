// Only the model is synthetic. Loader, AgentLoop, persistence, subprocesses
// and Git in the acceptance scenarios are the installed DSH implementation.
import { access, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { LlmAdapter } from '@deepseek-ai/dsh-llm';

export const name = 'worktrunk-gated-llm-fixture';
export const inject = ['llm'];

export function apply(ctx) {
  let request = 0;
  class GatedAdapter extends LlmAdapter {
    async resolveModel(provider, model) {
      return { provider, id: model, name: model, reasoning: {
        efforts: [{ id: 'off', name: 'Off' }], defaultEffort: 'off',
      } };
    }
    async * stream(options) {
      const id = `${process.env.WT_DSH_GATE_TAG}-${++request}`;
      const directory = process.env.WT_DSH_GATE_DIR;
      await writeFile(join(directory, `${id}.requested.json`), JSON.stringify({ messages: options.messages }));
      const deadline = Date.now() + 15000;
      while (process.env.WT_DSH_GATE_AUTO !== '1') {
        options.signal?.throwIfAborted();
        try { await access(join(directory, `${id}.release`)); break; }
        catch (error) { if (error.code !== 'ENOENT') throw error; }
        if (Date.now() >= deadline) throw new Error(`Unreleased synthetic model request ${id}`);
        await new Promise(resolve => setTimeout(resolve, 10));
      }
      const text = `Synthetic reply ${id}`;
      yield { type: 'block-start', index: 0, blockType: 'text' };
      yield { type: 'text-delta', index: 0, text };
      yield { type: 'block-end', index: 0, block: { type: 'text', text } };
      yield { type: 'usage', usage: { inputTokens: 3, outputTokens: 2 } };
      yield { type: 'finish', reason: { kind: 'stop' } };
    }
  }
  ctx.llm.registerAdapter(['worktrunk-fixture'], new GatedAdapter());
}
