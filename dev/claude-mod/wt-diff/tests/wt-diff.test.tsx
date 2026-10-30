import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

import { parsePatch } from '../hooks/patch'

const PATCH = [
  'diff --git a/src/lib.rs b/src/lib.rs',
  'index 1111111..2222222 100644',
  '--- a/src/lib.rs',
  '+++ b/src/lib.rs',
  '@@ -1,2 +1,2 @@',
  ' fn a() {}',
  '-fn b() {}',
  '+fn c() {}',
  'diff --git a/new.txt b/new.txt',
  'new file mode 100644',
  'index 0000000..3333333',
  '--- /dev/null',
  '+++ b/new.txt',
  '@@ -0,0 +1 @@',
  '+hello\r',
  'diff --git a/old.txt b/old.txt',
  'deleted file mode 100644',
  'index 4444444..0000000',
  '--- a/old.txt',
  '+++ /dev/null',
  '@@ -1 +0,0 @@',
  '-bye',
  'diff --git a/logo.png b/logo.png',
  'index 5555555..6666666 100644',
  'Binary files a/logo.png and b/logo.png differ',
  'diff --git a/x.md b/y.md',
  'similarity index 100%',
  'rename from x.md',
  'rename to y.md',
  '',
].join('\n')

test('a patch splits into its files', async () => {
  const files = parsePatch(PATCH)
  expect(files.map(f => [f.path, f.status, f.added, f.deleted])).toEqual([
    ['src/lib.rs', 'modified', 1, 1],
    ['new.txt', 'added', 1, 0],
    ['old.txt', 'deleted', 0, 1],
    ['logo.png', 'binary', 0, 0],
    ['y.md', 'renamed', 0, 0],
  ])
  // A CRLF file's carriage return is no control character a drawing takes.
  expect(files[1]?.hunks).toBe('@@ -0,0 +1 @@\n+hello�')
  expect(files[3]?.hunks).toBe('')
})

const HERE = '/r/wt'
const OTHER = '/r/wt.feature'

const LIST = JSON.stringify({
  schema: 1,
  repo: { default_branch: 'main' },
  items: [
    {
      branch: 'main',
      worktree: { path: HERE, main: true, changes: { diff: { added: 0, deleted: 0 } } },
    },
    {
      branch: 'feature',
      worktree: { path: OTHER, main: false, changes: { diff: { added: 2, deleted: 1 } } },
      default_branch: { ahead: 3, behind: 0, diff: { added: 40, deleted: 5 } },
    },
    { branch: 'unrelated', worktree: { path: '/r/wt.unrelated', main: false } },
  ],
})

const OPEN_PANE = {
  id: 'wt-diff',
  title: 'wt step diff',
  isShown: true,
  isFocused: true,
  isPlaced: true,
}

// What the engine stamps on a command the person typed.
const TYPED = {
  origin: { kind: 'composer' },
  presentation: { isFullscreen: true, columns: 160 },
} as const

const PANE = {
  plugin: 'wt-diff',
  component: 'Pane',
  requestId: 'wt-diff',
  props: {
    title: 'wt step diff',
    isFocused: true,
    bodyColumns: 80,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 40 },
    view: {},
  },
} as const

// Stands for the engine and the host: git and wt answer from fixtures.
function host(on: On, runs: string[][], diffError?: string) {
  mock.clock(on)
  on('command.register', () => ({ value: { command: 'wt-diff' } }))
  let isOpen = false
  on('ui.open', () => {
    isOpen = true
    return { value: { isPlaced: true } }
  })
  on('ui.panes', () => ({ value: isOpen ? [OPEN_PANE] : [] }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.cwd', () => ({ value: HERE }))
  on('tool.call', () => ({ result: {} }))
  on('process.run', (_, e) => {
    const argv = [...e.argv]
    runs.push(argv)
    const dir = argv[2] ?? ''
    const answer = (stdout: string) => ({
      value: { exitCode: 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
    })
    if (argv[0] === 'git') {
      return answer(`${dir.startsWith(OTHER) ? OTHER : HERE}\n${HERE}/.git\n`)
    }
    if (argv[3] === 'step' && diffError !== undefined) {
      return {
        value: {
          exitCode: 1,
          stdout: '',
          stderr: diffError,
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    }
    return answer(argv[3] === 'list' ? LIST : PATCH)
  })
}

test('the pane lists the worktrees the session worked in, and draws one diff', async ($, on) => {
  const runs: string[][] = []
  host(on, runs)

  await $.session.start({ cwd: HERE, surface: 'terminal', isInteractive: true })
  await $.tool.call({
    tool: 'Edit',
    file_path: `${OTHER}/src/lib.rs`,
    old_string: 'b',
    new_string: 'c',
  })
  await $.command.run({ command: 'wt-diff', args: 'feature', ...TYPED })

  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface })

    // Both worktrees the session touched, and not the one it never did.
    expect(await ui.find({ key: `wt:${HERE}` })).toBeDefined()
    const feature = await ui.find({ key: `wt:${OTHER}` })
    expect(feature?.text).toMatch(/feature.*↑3 ↓0 · committed \+40 -5 · uncommitted \+2 -1/)
    expect(await ui.find({ key: 'wt:/r/wt.unrelated' })).toBeUndefined()

    // The selected worktree's files, each drawn as a diff unless it has no hunks.
    expect((await ui.find({ key: 'file:src/lib.rs' }))?.text).toMatch(/M src\/lib\.rs \+1 -1/)
    const codes = await ui.findAll({ type: 'Code' })
    expect(codes.length).toBe(3)
    expect(await ui.find({ type: 'Text', text: 'Binary file.' })).toBeDefined()
    await ui.unmount()
  }

  // The diff came from Worktrunk, rooted at the selected worktree.
  expect(runs).toContainEqual([
    'wt',
    '-C',
    OTHER,
    'step',
    'diff',
    '--',
    '--no-color',
    '--no-ext-diff',
    '--src-prefix=a/',
    '--dst-prefix=b/',
  ])
})

test('a worktree whose diff fails shows why', async ($, on) => {
  host(on, [], '\u001b[31m✗\u001b[39m No common ancestor with target branch')

  await $.session.start({ cwd: HERE, surface: 'terminal', isInteractive: true })
  await $.command.run({ command: 'wt-diff', args: '', ...TYPED })
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect((await ui.find({ type: 'Text', text: /common ancestor/ }))?.text).toBe(
    '✗ No common ancestor with target branch',
  )
  await ui.unmount()
})
