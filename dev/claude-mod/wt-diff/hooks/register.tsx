import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register, Timer } from 'claude-code'

import type { DiffView, FileDiff, Row } from '../types'
import { parsePatch, printable } from './patch'

/**
 * A pane showing `wt step diff` for each worktree this session worked in.
 *
 * The session counts a worktree as worked in when it starts there, when its
 * shell's directory is in it after a Bash call, when a Bash call changes a file
 * in it, or when Edit, Write or NotebookEdit write a file in it, subagents'
 * edits included. `/wt-diff <branch>` adds a worktree by its branch.
 *
 * Worktrunk computes everything shown: `wt list --format=json` the rows, one
 * call per repository, and `wt step diff` the selected worktree's files. The
 * pane refreshes when it opens, when a worktree is selected, after each tool
 * call that may have written (debounced) and at the end of each turn.
 */

const PANE = 'wt-diff'
// Characters of hunks drawn at once; a drawing draws its first 100,000.
const BUDGET = 80_000

const touched = atom({ plugin: 'wt-diff', key: 'touched' } as const, [], {
  shape: 'touched-1',
})
const rows = atom({ plugin: 'wt-diff', key: 'rows' } as const, [], {
  shape: 'rows-1',
})
const current = atom({ plugin: 'wt-diff', key: 'current' } as const, null)
const selected = atom({ plugin: 'wt-diff', key: 'selected' } as const, null)
const diffs = atom({ plugin: 'wt-diff', key: 'diffs' } as const, {}, {
  shape: 'diffs-1',
})
const cursor = atom({ plugin: 'wt-diff', key: 'cursor' } as const, 0)

type Located = { root: string; commonDir: string }

// Per load: a reload asks git again and drops a pending refresh.
const located = new Map<string, Located | null>()
let refreshTimer: Timer | undefined
let refreshing: Promise<void> = Promise.resolve()

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'wt-diff',
      description: 'Show `wt step diff` for the worktrees this session worked in',
      argumentHint: '[branch]',
      immediate: true,
    })
    const started = await next(e)
    await track($, [e.cwd])
    return started
  })

  on('command.run', { command: 'wt-diff' }, async ($, e) => {
    const branch = e.args.trim()
    const cwd = await $.session.cwd()
    const root =
      branch === ''
        ? ((await locate($, cwd))?.root ?? null)
        : await worktreeOf($, cwd, branch)
    if (root === null) {
      return {
        text:
          branch === ''
            ? `wt-diff: ${cwd} is in no git worktree`
            : `wt-diff: no worktree for branch ${branch}`,
      }
    }
    await track($, [root])
    await select($, root)
    const opened = await $.ui.open({ id: PANE, title: 'wt step diff', focus: true })
    await refresh($)
    return opened.isPlaced ? {} : { text: 'wt-diff: the pane is open but not shown here.' }
  })

  // It judges nothing: where it fails, the call goes on as it ran.
  on('tool.call', async ($, e, next) => {
    const ran = await next(e)
    if (ran.deny !== undefined) {
      return ran
    }
    const dirs: string[] = []
    if (e.tool === 'Edit' || e.tool === 'Write') {
      dirs.push(parentOf(e.file_path))
    } else if (e.tool === 'NotebookEdit') {
      dirs.push(parentOf(e.notebook_path))
    } else if (e.tool === 'Bash') {
      // A subagent's shell has a directory of its own, which the session's
      // is not; its writes still show in what the command changed.
      if (e.agentId === undefined) {
        dirs.push(await $.session.cwd())
      }
      dirs.push(...changedFiles(ran.result).map(parentOf))
    } else if (e.tool === 'EnterWorktree' || e.tool === 'ExitWorktree') {
      dirs.push(await $.session.cwd())
    } else {
      return ran
    }
    await track($, dirs)
    return ran
  }).catch(($, e, next) => next(e))

  on('turn.complete', async ($, e, next) => {
    const done = await next(e)
    if (await isPaneOpen($)) {
      void refresh($)
    }
    return done
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Button, Code, Text } = $.ui.resolve(e)
    const [list, known, here, root, all, at] = await Promise.all([
      read($, touched),
      read($, rows),
      read($, current),
      read($, selected),
      read($, diffs),
      read($, cursor),
    ])

    // The session's own worktree first, then the most recently worked in.
    const ordered = [...list].sort(
      (a, b) =>
        Number(b.root === here) - Number(a.root === here) || b.lastAt - a.lastAt,
    )
    const rowOf = (one: string) => known.find(row => row.root === one)
    const view = root === null ? undefined : all[root]
    const row = root === null ? undefined : rowOf(root)
    const files = view?.files ?? []
    const drawn = drawnFiles(files, at)

    return (
      <Box flexDirection="column">
        <Text bold>Worktrees this session worked in</Text>
        {ordered.length === 0 && <Text dimColor>None yet.</Text>}
        {ordered.map((one, index) => {
          const info = rowOf(one.root)
          return (
            <Button
              key={`wt:${one.root}`}
              plain
              hotkey={index < 9 ? String(index + 1) : undefined}
              onPress={() => pick($, one.root)}
            >
              {one.root === root ? '▸ ' : '  '}
              {info?.branch ?? baseName(one.root)}
              {one.root === here ? ' @' : ''}{' '}
              <Text dimColor>{info === undefined ? '' : summary(info)}</Text>
            </Button>
          )
        })}
        <Text dimColor>{'─'.repeat(Math.max(10, e.props.bodyColumns))}</Text>
        {root === null ? (
          <Text dimColor>Select a worktree.</Text>
        ) : (
          <Box flexDirection="column">
            <Text>
              <Text bold>{row?.branch ?? baseName(root)}</Text>
              <Text dimColor>
                {' '}
                vs {row?.target ?? 'default branch'} · {files.length} files ·{' '}
              </Text>
              <Text color="diffAdded">+{sum(files, 'added')}</Text>{' '}
              <Text color="diffRemoved">-{sum(files, 'deleted')}</Text>
            </Text>
            <Box flexDirection="row" gap={2}>
              <Button key="refresh" plain hotkey="r" label="refresh" onPress={() => refresh($)} />
              <Button key="next" plain hotkey="n" label="next file" onPress={() => move($, 1)} />
              <Button key="prev" plain hotkey="p" label="previous" onPress={() => move($, -1)} />
            </Box>
            {view === undefined && <Text dimColor>Loading…</Text>}
            {view !== undefined && view.error !== null && (
              <Text color="error">{view.error}</Text>
            )}
            {view !== undefined && view.error === null && files.length === 0 && (
              <Text dimColor>No changes since the merge base.</Text>
            )}
            {files.map((file, index) => (
              <Button
                key={`file:${file.path}`}
                plain
                dimColor={index !== at}
                onPress={() => jump($, index, file.path)}
              >
                {glyph(file)} {file.path}{' '}
                <Text color="diffAdded">+{file.added}</Text>{' '}
                <Text color="diffRemoved">-{file.deleted}</Text>
              </Button>
            ))}
            {files.map((file, index) => (
              <Box key={`box:${file.path}`} flexDirection="column" marginTop={1}>
                <Button
                  key={`hunk:${file.path}`}
                  plain
                  onPress={() => backToTop($, index)}
                >
                  <Text bold>
                    {glyph(file)} {file.path}
                  </Text>
                </Button>
                {!drawn.has(index) ? (
                  <Text dimColor>
                    {file.added + file.deleted} lines not drawn; select the file to draw it.
                  </Text>
                ) : file.hunks === '' ? (
                  <Text dimColor>
                    {file.status === 'binary' ? 'Binary file.' : 'No line changes.'}
                  </Text>
                ) : (
                  <Code source={file.hunks} format="diff" path={file.path} />
                )}
              </Box>
            ))}
          </Box>
        )}
      </Box>
    )
  })
}

async function locate($: EngineInterface, dir: string): Promise<Located | null> {
  const known = located.get(dir)
  if (known !== undefined) {
    return known
  }
  // A deleted file's directory may be gone; its nearest parent is not.
  for (let at = dir, tries = 0; tries < 4; at = parentOf(at), tries += 1) {
    const ran = await $.process
      .run([
        'git',
        '-C',
        at,
        'rev-parse',
        '--path-format=absolute',
        '--show-toplevel',
        '--git-common-dir',
      ])
      .catch(() => null)
    if (ran === null) {
      break
    }
    if (ran.exitCode === 0) {
      const [root, commonDir] = ran.stdout.trim().split('\n')
      const found = root && commonDir ? { root, commonDir } : null
      located.set(dir, found)
      return found
    }
    if (!/cannot change to/.test(ran.stderr)) {
      break
    }
  }
  located.set(dir, null)
  return null
}

async function track($: EngineInterface, dirs: readonly string[]) {
  const now = await $.clock.now()
  for (const dir of dirs) {
    const found = await locate($, dir)
    if (found === null) {
      continue
    }
    await update($, touched, list => [
      ...list.filter(one => one.root !== found.root),
      { ...found, lastAt: now },
    ])
  }
  const here = await locate($, await $.session.cwd())
  await update($, current, () => here?.root ?? null)
  // What the call changed shows once a burst of calls goes quiet.
  refreshTimer?.cancel()
  refreshTimer = $.clock.after(400, () => {
    void refresh($)
  })
}

// One refresh at a time; a request during one runs after it.
function refresh($: EngineInterface): Promise<void> {
  refreshTimer?.cancel()
  refreshing = refreshing.then(async () => {
    if (!(await isPaneOpen($))) {
      return
    }
    await refreshRows($)
    const root = await read($, selected)
    if (root !== null) {
      await refreshDiff($, root)
    }
  })
  return refreshing
}

async function isPaneOpen($: EngineInterface): Promise<boolean> {
  return (await $.ui.panes()).some(pane => pane.id === PANE)
}

async function refreshRows($: EngineInterface) {
  const list = await read($, touched)
  const repos = new Map<string, string[]>()
  for (const one of list) {
    repos.set(one.commonDir, [...(repos.get(one.commonDir) ?? []), one.root])
  }
  const found: Row[] = []
  for (const roots of repos.values()) {
    const [first] = roots
    if (first === undefined) {
      continue
    }
    const ran = await $.process
      .run(['wt', '-C', first, 'list', '--format=json'], { timeoutMs: 60_000 })
      .catch(() => null)
    if (ran !== null && ran.exitCode === 0) {
      found.push(...readRows(ran.stdout, roots))
    }
  }
  await update($, rows, () => found)
}

async function refreshDiff($: EngineInterface, root: string) {
  const ran = await $.process
    .run(
      [
        'wt',
        '-C',
        root,
        'step',
        'diff',
        '--',
        '--no-color',
        '--no-ext-diff',
        '--src-prefix=a/',
        '--dst-prefix=b/',
      ],
      { timeoutMs: 60_000 },
    )
    .catch((error: unknown) => ({ exitCode: -1, stdout: '', stderr: String(error) }))
  const view: DiffView = {
    fetchedAt: await $.clock.now(),
    files: ran.exitCode === 0 ? parsePatch(ran.stdout) : [],
    error:
      ran.exitCode === 0
        ? null
        : printable(stripAnsi(ran.stderr).trim()) || `wt step diff exited ${ran.exitCode}`,
  }
  await update($, diffs, all => ({ ...all, [root]: view }))
}

async function select($: EngineInterface, root: string) {
  await update($, selected, () => root)
  await update($, cursor, () => 0)
}

// A pick in the pane also brings the new worktree's top into view, before
// the refresh, so a scroll the person makes while it runs stands.
async function pick($: EngineInterface, root: string) {
  await select($, root)
  await $.ui.scroll({ to: 'start', in: PANE })
  await refresh($)
}

async function move($: EngineInterface, by: number) {
  const root = await read($, selected)
  const files = root === null ? [] : ((await read($, diffs))[root]?.files ?? [])
  if (files.length === 0) {
    return
  }
  const at = await update($, cursor, n => Math.min(files.length - 1, Math.max(0, n + by)))
  const file = files[at]
  if (file !== undefined) {
    await $.ui.scroll({ to: { key: `hunk:${file.path}` }, in: PANE, block: 'start' })
  }
}

async function jump($: EngineInterface, index: number, path: string) {
  await update($, cursor, () => index)
  await $.ui.scroll({ to: { key: `hunk:${path}` }, in: PANE, block: 'start' })
}

async function backToTop($: EngineInterface, index: number) {
  await update($, cursor, () => index)
  await $.ui.scroll({ to: 'start', in: PANE })
}

/** The worktree root of `branch` in the repository at `cwd`, from `wt list`. */
async function worktreeOf(
  $: EngineInterface,
  cwd: string,
  branch: string,
): Promise<string | null> {
  const ran = await $.process
    .run(['wt', '-C', cwd, 'list', '--format=json'], { timeoutMs: 60_000 })
    .catch(() => null)
  if (ran === null || ran.exitCode !== 0) {
    return null
  }
  try {
    const parsed = JSON.parse(ran.stdout) as {
      items?: { branch?: string | null; worktree?: { path?: string } }[]
    }
    return parsed.items?.find(item => item.branch === branch)?.worktree?.path ?? null
  } catch {
    return null
  }
}

/** The files whose hunks fit the budget: the one at `at` first, then in order. */
function drawnFiles(files: readonly FileDiff[], at: number): Set<number> {
  const drawn = new Set<number>()
  let used = 0
  for (const index of [at, ...files.keys()]) {
    const size = files[index]?.hunks.length
    if (size !== undefined && !drawn.has(index) && used + size <= BUDGET) {
      drawn.add(index)
      used += size
    }
  }
  return drawn
}

function glyph(file: FileDiff): string {
  return { added: 'A', deleted: 'D', renamed: 'R', modified: 'M', binary: 'B' }[file.status]
}

function sum(files: readonly FileDiff[], key: 'added' | 'deleted'): number {
  return files.reduce((total, file) => total + file[key], 0)
}

function summary(row: Row): string {
  const parts: string[] = []
  if (row.isMain) {
    parts.push('main worktree')
  }
  if (row.ahead > 0 || row.behind > 0) {
    parts.push(`↑${row.ahead} ↓${row.behind}`)
  }
  const { committed: c, uncommitted: u } = row
  if (c.added + c.deleted > 0) {
    parts.push(`committed +${c.added} -${c.deleted}`)
  }
  if (u.added + u.deleted > 0) {
    parts.push(`uncommitted +${u.added} -${u.deleted}`)
  }
  return parts.join(' · ')
}

/** The rows of `wt list --format=json` output for the worktrees at `roots`. */
function readRows(json: string, roots: readonly string[]): Row[] {
  type Item = {
    branch?: string | null
    worktree?: {
      path?: string
      main?: boolean
      changes?: { diff?: { added?: number; deleted?: number } }
    }
    default_branch?: {
      ahead?: number
      behind?: number
      diff?: { added?: number; deleted?: number }
    }
  }
  let parsed: { repo?: { default_branch?: string }; items?: Item[] }
  try {
    parsed = JSON.parse(json)
  } catch {
    return []
  }
  const target = parsed.repo?.default_branch ?? null
  return (parsed.items ?? []).flatMap(item => {
    const path = item.worktree?.path
    if (path === undefined || !roots.includes(path)) {
      return []
    }
    const uncommitted = item.worktree?.changes?.diff
    const committed = item.default_branch?.diff
    return [
      {
        root: path,
        branch: item.branch ?? null,
        target,
        ahead: item.default_branch?.ahead ?? 0,
        behind: item.default_branch?.behind ?? 0,
        committed: { added: committed?.added ?? 0, deleted: committed?.deleted ?? 0 },
        uncommitted: { added: uncommitted?.added ?? 0, deleted: uncommitted?.deleted ?? 0 },
        isMain: item.worktree?.main === true,
      },
    ]
  })
}

/** The absolute paths a Bash call changed, where the engine recorded them. */
function changedFiles(result: unknown): string[] {
  const paths = (result as { bashEditDiff?: { changedFiles?: unknown } } | undefined)
    ?.bashEditDiff?.changedFiles
  return Array.isArray(paths) ? paths.filter(p => typeof p === 'string') : []
}

function parentOf(path: string): string {
  return path.replace(/\/[^/]*\/?$/, '') || '/'
}

function baseName(path: string): string {
  return path.replace(/\/$/, '').replace(/^.*\//, '')
}

function stripAnsi(text: string): string {
  // CSI sequences, and OSC 8 hyperlinks ending in ESC \.
  return text.replace(/\u001b\[[0-9;?]*[ -/]*[@-~]|\u001b\][^\u001b]*\u001b\\/g, '')
}
