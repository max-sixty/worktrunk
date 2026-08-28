import assert from 'node:assert/strict';
import test from 'node:test';

import {
  rehypeStableHeadingIds,
  slugHeading,
} from '../src/plugins/stable-heading-ids.mjs';
import { rehypePagefindCommandReferences } from '../src/plugins/pagefind-command-references.mjs';
import { rehypeResponsiveTables } from '../src/plugins/responsive-tables.mjs';
import {
  commandReferenceSegments,
  pluginWorktrunkTerminal,
  rehypeComparisonCommands,
  semanticOutputSegments,
  shellCommandSegments,
} from '../src/plugins/worktrunk-terminal.mjs';

function prepareCodeBlock(plugin, codeBlock) {
  codeBlock.props ??= {};
  plugin.hooks.preprocessLanguage({ codeBlock });
  plugin.hooks.preprocessCode({ codeBlock });
}

function markdownTable(headers, rows, { className, headerProperties } = {}) {
  const row = (tagName, values, properties = []) => ({
    type: 'element',
    tagName: 'tr',
    properties: {},
    children: values.map((value, index) => ({
      type: 'element',
      tagName,
      properties: properties[index] ?? {},
      children: [{ type: 'text', value }],
    })),
  });
  return {
    type: 'element',
    tagName: 'table',
    properties: className ? { className } : {},
    children: [
      {
        type: 'element',
        tagName: 'thead',
        properties: {},
        children: [row('th', headers, headerProperties)],
      },
      {
        type: 'element',
        tagName: 'tbody',
        properties: {},
        children: rows.map((values) => row('td', values)),
      },
    ],
  };
}

test('heading slugs preserve existing public anchors', () => {
  const headings = new Map([
    ['Example: CI/testing override', 'example-ci-testing-override'],
    ['Inline config overrides (--config-set)', 'inline-config-overrides-config-set'],
    ['What’s cached', 'what-s-cached'],
    ['What’s logged', 'what-s-logged'],
    ['Command log (commands.jsonl)', 'command-log-commands-jsonl'],
    ['vs. git-machete / git-town', 'vs-git-machete-git-town'],
    ["There's an issue with my shell setup", 'there-s-an-issue-with-my-shell-setup'],
    ['What does -v / -vv do?', 'what-does-v-vv-do'],
    [
      'My for-each or --execute alias prints the same value in every worktree',
      'my-for-each-or-execute-alias-prints-the-same-value-in-every-worktree',
    ],
    ['working_tree object', 'working-tree-object'],
    ['main_state values', 'main-state-values'],
    ['integration_reason values', 'integration-reason-values'],
    ['ci.status and ci.review_state values', 'ci-status-and-ci-review-state-values'],
    ['Node.js', 'node-js'],
    ['Shell alias for new worktree + agent', 'shell-alias-for-new-worktree-agent'],
  ]);
  for (const [heading, expected] of headings) {
    assert.equal(slugHeading(heading), expected);
  }
});

test('duplicate heading slugs are unique', () => {
  const headings = [
    { type: 'element', tagName: 'h2', properties: {}, children: [{ type: 'text', value: 'Demo' }] },
    { type: 'element', tagName: 'h2', properties: {}, children: [{ type: 'text', value: 'Demo' }] },
  ];
  rehypeStableHeadingIds()({ type: 'root', children: headings });
  assert.deepEqual(headings.map((heading) => heading.properties.id), ['demo', 'demo-1']);
});

function heading(tagName, value) {
  return {
    type: 'element',
    tagName,
    properties: {},
    children: [{ type: 'text', value }],
  };
}

test('nested command references receive hidden Pagefind context', () => {
  const command = heading('h2', 'wt step commit');
  const reference = heading('h3', 'Command reference');
  const tree = { type: 'root', children: [command, reference] };

  rehypePagefindCommandReferences()(tree);
  rehypePagefindCommandReferences()(tree);

  assert.deepEqual(reference.children, [
    {
      type: 'raw',
      value: '<span class="wt-pagefind-fragment-title" hidden aria-hidden="true">wt step commit — </span>',
    },
    { type: 'text', value: 'Command reference' },
  ]);
});

test('Pagefind context rejects non-command and non-nested references', () => {
  const topLevelReference = heading('h2', 'Command reference');
  const orphanReference = heading('h3', 'Command reference');
  const command = heading('h2', 'wt step commit');
  const otherSubheading = heading('h3', 'Examples');
  const unrelated = heading('h2', 'Configuration');
  const unrelatedReference = heading('h3', 'Command reference');
  const tree = {
    type: 'root',
    children: [
      topLevelReference,
      orphanReference,
      command,
      otherSubheading,
      unrelated,
      unrelatedReference,
    ],
  };

  rehypePagefindCommandReferences()(tree);

  for (const candidate of [
    topLevelReference,
    orphanReference,
    otherSubheading,
    unrelatedReference,
  ]) {
    assert.deepEqual(candidate.children, [{ type: 'text', value: candidate.children[0].value }]);
  }
});

test('short wide tables become accessible responsive records', () => {
  const table = markdownTable(
    ['File', 'Location', 'Contains'],
    [
      ['User config', '~/.config/worktrunk/config.toml', 'Personal settings'],
      ['Project config', '.config/wt.toml', 'Shared hooks'],
    ],
  );

  rehypeResponsiveTables()({ type: 'root', children: [table] });

  assert.deepEqual(table.properties, {
    className: ['wt-responsive-records'],
    dataRecordColumns: '3',
  });
  const cells = table.children[1].children.flatMap((row) => row.children);
  assert.deepEqual(
    cells.map((cell) => cell.children[0]),
    ['File', 'Location', 'Contains', 'File', 'Location', 'Contains'].map((value) => ({
      type: 'element',
      tagName: 'span',
      properties: {
        ariaHidden: 'true',
        className: ['wt-responsive-record-label'],
      },
      children: [{ type: 'text', value }],
    })),
  );
  assert.deepEqual(
    cells.map((cell) => cell.children[1].value),
    ['User config', '~/.config/worktrunk/config.toml', 'Personal settings', 'Project config', '.config/wt.toml', 'Shared hooks'],
  );
});

test('responsive records reject tables that cannot stack unambiguously', () => {
  const twoColumns = markdownTable(['Name', 'Meaning'], [['one', 'first']]);
  const dense = markdownTable(
    ['Name', 'Value', 'Meaning'],
    Array.from({ length: 4 }, (_, index) => [`name ${index}`, `${index}`, `meaning ${index}`]),
  );
  const comparison = markdownTable(
    ['Task', 'Worktrunk', 'Plain git'],
    [['Switch', 'wt switch', 'git worktree add']],
    { className: ['cmd-compare'] },
  );
  const complexHeader = markdownTable(
    ['Grouped', 'Value', 'Meaning'],
    [['one', '1', 'first']],
    { headerProperties: [{ colSpan: 2 }] },
  );
  const emptyHeader = markdownTable(['Name', '', 'Meaning'], [['one', '1', 'first']]);
  const duplicateHeader = markdownTable(['Name', 'Value', 'Value'], [['one', '1', 'first']]);
  const mismatchedRow = markdownTable(['Name', 'Value', 'Meaning'], [['one', '1']]);
  const multipleHeaderRows = markdownTable(['Name', 'Value', 'Meaning'], [['one', '1', 'first']]);
  multipleHeaderRows.children[0].children.push(structuredClone(multipleHeaderRows.children[0].children[0]));
  const tables = [
    twoColumns,
    dense,
    comparison,
    complexHeader,
    emptyHeader,
    duplicateHeader,
    mismatchedRow,
    multipleHeaderRows,
  ];

  rehypeResponsiveTables()({ type: 'root', children: tables });

  for (const table of tables) {
    assert.ok(!table.properties.className?.includes('wt-responsive-records'));
    assert.equal(
      table.children[1].children.flatMap((row) => row.children)
        .filter((cell) => cell.children[0]?.properties?.className?.includes('wt-responsive-record-label'))
        .length,
      0,
    );
  }
});

test('homepage comparison commands gain shell roles without changing their text', () => {
  const command = {
    type: 'element',
    tagName: 'code',
    properties: {},
    children: [{
      type: 'text',
      value: 'git worktree add -b feat ../repo.feat && \\\ncd ../repo.feat',
    }],
  };
  const outsideCode = structuredClone(command);
  const comparison = {
    type: 'element',
    tagName: 'table',
    properties: { className: ['cmd-compare'] },
    children: [{
      type: 'element',
      tagName: 'tbody',
      properties: {},
      children: [{
        type: 'element',
        tagName: 'tr',
        properties: {},
        children: [{
          type: 'element',
          tagName: 'td',
          properties: {},
          children: [command],
        }],
      }],
    }],
  };
  const tree = { type: 'root', children: [outsideCode, comparison] };

  rehypeComparisonCommands()(tree);

  assert.deepEqual(shellCommandSegments(command.children.map((child) => (
    child.children?.[0]?.value ?? child.value
  )).join('')), [
    { text: 'git', tone: 'command' },
    { text: ' ' },
    { text: 'worktree', tone: 'argument' },
    { text: ' ' },
    { text: 'add', tone: 'argument' },
    { text: ' ' },
    { text: '-b', tone: 'option' },
    { text: ' ' },
    { text: 'feat', tone: 'argument' },
    { text: ' ' },
    { text: '../repo.feat', tone: 'argument' },
    { text: ' && \\\n' },
    { text: 'cd', tone: 'command' },
    { text: ' ' },
    { text: '../repo.feat', tone: 'argument' },
  ]);
  assert.equal(command.children.map((child) => child.children?.[0]?.value ?? child.value).join(''),
    'git worktree add -b feat ../repo.feat && \\\ncd ../repo.feat');
  assert.deepEqual(
    command.children.filter((child) => child.type === 'element')
      .map((child) => child.properties.className[0]),
    [
      'wt-shell-command',
      'wt-shell-argument',
      'wt-shell-argument',
      'wt-shell-option',
      'wt-shell-argument',
      'wt-shell-argument',
      'wt-shell-command',
      'wt-shell-argument',
    ],
  );
  assert.deepEqual(outsideCode, structuredClone({
    type: 'element',
    tagName: 'code',
    properties: {},
    children: [{
      type: 'text',
      value: 'git worktree add -b feat ../repo.feat && \\\ncd ../repo.feat',
    }],
  }));
});

test('raw homepage comparison markup is expanded before shell roles are added', () => {
  const source = 'wt switch -c feat';
  const tree = {
    type: 'root',
    children: [{
      type: 'raw',
      value: '<table class="cmd-compare"><tbody><tr><td><code>'
        + source
        + '</code></td></tr></tbody></table>',
    }],
  };

  rehypeComparisonCommands()(tree);

  const table = tree.children[0];
  const code = table.children[0].children[0].children[0].children[0];
  assert.equal(table.type, 'element');
  assert.equal(code.children.map((child) => child.children?.[0]?.value ?? child.value).join(''), source);
  assert.deepEqual(
    code.children.filter((child) => child.type === 'element')
      .map((child) => child.properties.className[0]),
    ['wt-shell-command', 'wt-shell-argument', 'wt-shell-option', 'wt-shell-argument'],
  );
});

test('shell command roles preserve syntax outside the styled grammar', () => {
  const source = 'cmd & next\ncmd 2>&1';
  assert.equal(shellCommandSegments(source).map(({ text }) => text).join(''), source);
});

test('console blocks separate copyable recipes from output', () => {
  const lines = ['# Recent', '$ wt list', '', '# Failed', '$ wt list --full'].map((text) => ({
    text,
    editText(start, end, replacement) {
      this.text = this.text.slice(0, start) + replacement + this.text.slice(end);
    },
  }));
  const codeBlock = { language: 'console', getLines: () => lines };
  const plugin = pluginWorktrunkTerminal();
  prepareCodeBlock(plugin, codeBlock);

  assert.equal(codeBlock.language, 'bash');
  assert.equal(codeBlock.props.frame, 'terminal');

  const classes = lines.map((_, lineIndex) => {
    const renderData = { lineAst: { properties: {} } };
    plugin.hooks.postprocessRenderedLine({ codeBlock, lineIndex, renderData });
    return renderData.lineAst.properties.className;
  });

  assert.deepEqual(
    lines.map((line) => line.text),
    ['# Recent', 'wt list', '', '# Failed', 'wt list --full'],
  );
  assert.deepEqual(classes, [
    ['wt-copyable'],
    ['wt-command'],
    ['wt-copyable'],
    ['wt-copyable'],
    ['wt-command'],
  ]);

  const copyButton = { type: 'element', tagName: 'button', properties: { 'data-code': 'stale' } };
  const renderData = {
    blockAst: {
      children: [{
        type: 'element',
        properties: { className: ['copy'] },
        children: [copyButton],
      }],
    },
  };
  plugin.hooks.postprocessRenderedBlock({ codeBlock, renderData });
  assert.equal(
    copyButton.properties['data-code'],
    '# Recent\u007fwt list\u007f\u007f# Failed\u007fwt list --full',
  );
});

test('console output and its blank lines stay out of copied commands', () => {
  const lines = ['$ wt list', 'output', '', '# shell comment'].map((text) => ({
    text,
    editText(start, end, replacement) {
      this.text = this.text.slice(0, start) + replacement + this.text.slice(end);
    },
  }));
  const codeBlock = { language: 'console', getLines: () => lines };
  const plugin = pluginWorktrunkTerminal();
  prepareCodeBlock(plugin, codeBlock);

  const classes = lines.map((_, lineIndex) => {
    const renderData = { lineAst: { properties: {} } };
    plugin.hooks.postprocessRenderedLine({ codeBlock, lineIndex, renderData });
    return renderData.lineAst.properties.className;
  });
  assert.deepEqual(classes, [
    ['wt-command'],
    ['wt-output'],
    ['wt-output'],
    ['wt-copyable'],
  ]);

  const copyButton = { type: 'element', tagName: 'button', properties: { 'data-code': 'stale' } };
  const renderData = {
    blockAst: {
      children: [{
        type: 'element',
        properties: { className: ['copy'] },
        children: [copyButton],
      }],
    },
  };
  plugin.hooks.postprocessRenderedBlock({ codeBlock, renderData });
  assert.equal(copyButton.properties['data-code'], 'wt list\u007f# shell comment');
});

test('shell snippets use the command-only terminal classifier', () => {
  const codeBlock = { language: 'bash', getLines: () => [] };
  const blockAst = {
    type: 'element',
    tagName: 'figure',
    properties: { className: ['frame', 'is-terminal'] },
    children: [],
  };
  const plugin = pluginWorktrunkTerminal();

  plugin.hooks.preprocessCode({ codeBlock });
  plugin.hooks.postprocessRenderedBlock({ codeBlock, renderData: { blockAst } });

  assert.deepEqual(blockAst.properties.className, [
    'frame',
    'is-terminal',
    'wt-commands-only',
  ]);
});

test('console output retains state-color semantics without ANSI in Markdown', () => {
  assert.deepEqual(
    semanticOutputSegments('@ feat  +54  -5  ↑4  ↓1  ⇡3  ?  ✓ done'),
    [
      { text: '@ feat  ' },
      { text: '+54', tone: 'positive' },
      { text: '  ' },
      { text: '-5', tone: 'negative' },
      { text: '  ' },
      { text: '↑4', tone: 'positive' },
      { text: '  ' },
      { text: '↓1', tone: 'negative' },
      { text: '  ' },
      { text: '⇡3', tone: 'positive' },
      { text: '  ' },
      { text: '?', tone: 'warning' },
      { text: '  ' },
      { text: '✓', tone: 'positive' },
      { text: ' done' },
    ],
  );

  assert.deepEqual(
    semanticOutputSegments('[unoptimized + debuginfo] Allow and remember?'),
    [{ text: '[unoptimized + debuginfo] Allow and remember?' }],
  );
  assert.deepEqual(
    semanticOutputSegments('release-5 v1+2 port -3000'),
    [{ text: 'release-5 v1+2 port -3000' }],
  );
  assert.deepEqual(
    semanticOutputSegments('@ feat  +   ↑'),
    [{ text: '@ feat  +   ↑' }],
  );
});

test('console commands retain syntax highlighter token spans', () => {
  const lines = ['$ wt switch "#412" # inspect the PR'].map((text) => ({
    text,
    editText(start, end, replacement) {
      this.text = this.text.slice(0, start) + replacement + this.text.slice(end);
    },
  }));
  const codeBlock = { language: 'console', getLines: () => lines };
  const plugin = pluginWorktrunkTerminal();
  prepareCodeBlock(plugin, codeBlock);

  const rendered = lines.map((line, lineIndex) => {
    const code = {
      type: 'element',
      tagName: 'div',
      properties: { className: ['code'] },
      children: [
        {
          type: 'element',
          tagName: 'span',
          properties: { style: '--0:#aaa;--1:#111' },
          children: [{ type: 'text', value: 'wt' }],
        },
        { type: 'text', value: ' switch "#412" ' },
        {
          type: 'element',
          tagName: 'span',
          properties: { style: '--0:#bbb;--1:#222' },
          children: [{ type: 'text', value: '# inspect the PR' }],
        },
      ],
    };
    const renderData = {
      lineAst: { type: 'element', tagName: 'div', properties: {}, children: [code] },
    };
    plugin.hooks.postprocessRenderedLine({ codeBlock, line, lineIndex, renderData });
    return code.children;
  });

  assert.deepEqual(rendered, [
    [
      {
        type: 'element',
        tagName: 'span',
        properties: { style: '--0:#aaa;--1:#111' },
        children: [{ type: 'text', value: 'wt' }],
      },
      { type: 'text', value: ' switch "#412" ' },
      {
        type: 'element',
        tagName: 'span',
        properties: { style: '--0:#bbb;--1:#222' },
        children: [{ type: 'text', value: '# inspect the PR' }],
      },
    ],
  ]);
});

test('command references expose clap syntax roles', () => {
  const plugin = pluginWorktrunkTerminal();
  const markedBlock = {
    language: 'text',
    metaOptions: { value: (key) => (key === 'wt-command-reference' ? true : undefined) },
  };
  const plainBlock = {
    language: 'text',
    metaOptions: { value: () => undefined },
  };
  plugin.hooks.preprocessLanguage({ codeBlock: markedBlock });
  plugin.hooks.preprocessLanguage({ codeBlock: plainBlock });
  for (const [codeBlock, expected] of [[markedBlock, true], [plainBlock, false]]) {
    const blockAst = { properties: { className: ['frame'] }, children: [] };
    plugin.hooks.postprocessRenderedBlock({ codeBlock, renderData: { blockAst } });
    assert.equal(blockAst.properties.className.includes('wt-command-reference'), expected);
  }

  assert.deepEqual(commandReferenceSegments('Usage: wt list [OPTIONS] <COMMAND>'), [
    { text: 'Usage:', tone: 'heading' },
    { text: ' ' },
    { text: 'wt list', tone: 'command' },
    { text: ' ' },
    { text: '[OPTIONS]', tone: 'value' },
    { text: ' ' },
    { text: '<COMMAND>', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('       wt list <COMMAND>'), [
    { text: '       ' },
    { text: 'wt list', tone: 'command' },
    { text: ' ' },
    { text: '<COMMAND>', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('      --format <FORMAT>'), [
    { text: '      ' },
    { text: '--format', tone: 'option' },
    { text: ' ' },
    { text: '<FORMAT>', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('          [default: table]'), [
    { text: '          ' },
    { text: '[default: table]', tone: 'meta' },
  ]);
  assert.deepEqual(commandReferenceSegments('  [EXTRA_ARGS]...'), [
    { text: '  ' },
    { text: '[EXTRA_ARGS]...', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('  <COMMAND>...'), [
    { text: '  ' },
    { text: '<COMMAND>...', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('          Possible values:'), [
    { text: '          ' },
    { text: 'Possible values:', tone: 'meta' },
  ]);
  assert.deepEqual(commandReferenceSegments('          - all: Stage everything'), [
    { text: '          - ' },
    { text: 'all', tone: 'value' },
    { text: ':' },
    { text: ' Stage everything' },
  ]);
  assert.deepEqual(commandReferenceSegments('          - json'), [
    { text: '          - ' },
    { text: 'json', tone: 'value' },
  ]);
  assert.deepEqual(commandReferenceSegments('wt step eval - [experimental] Evaluate a template'), [
    { text: 'wt step eval', tone: 'command' },
    { text: ' - ' },
    { text: '[experimental]', tone: 'meta' },
    { text: ' Evaluate a template' },
  ]);
  assert.deepEqual(commandReferenceSegments('  eval    [experimental] Evaluate a template'), [
    { text: '  ' },
    { text: 'eval', tone: 'command' },
    { text: '    ' },
    { text: '[experimental]', tone: 'meta' },
    { text: ' Evaluate a template' },
  ]);
});
