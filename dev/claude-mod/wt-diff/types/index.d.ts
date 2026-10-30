/** A worktree the session worked in, by its root. */
export type Touched = {
  root: string
  /** The repository's common git dir: worktrees sharing it share one `wt list`. */
  commonDir: string
  lastAt: number
}

/** Lines added and deleted. */
export type Lines = { added: number; deleted: number }

/** What `wt list --format=json` says of one worktree, as the pane shows it. */
export type Row = {
  root: string
  branch: string | null
  target: string | null
  /** Commits ahead of and behind the default branch. */
  ahead: number
  behind: number
  /** `main…±`: committed changes since the merge base. */
  committed: Lines
  /** `HEAD±`: uncommitted changes, untracked files included. */
  uncommitted: Lines
  isMain: boolean
}

/** One file of a `wt step diff`. */
export type FileDiff = {
  path: string
  status: 'added' | 'deleted' | 'renamed' | 'modified' | 'binary'
  added: number
  deleted: number
  /** What the hunks don't show: `from <path>` for a rename, a mode change. */
  note: string | null
  /** The file's hunks from its first `@@`, empty for a binary or mode-only change. */
  hunks: string
}

/** One worktree's `wt step diff`, or why there is none. */
export type DiffView = {
  fetchedAt: number
  files: FileDiff[]
  /** Whether the patch passed what `$.process.run` captures, its last file dropped. */
  isTruncated: boolean
  error: string | null
}

declare module 'claude-code' {
  interface PluginState {
    'wt-diff': {
      touched: Shaped<Touched[]>
      rows: Shaped<Row[]>
      /** The root of the worktree the session's directory is in. */
      current: string | null
      selected: string | null
      diffs: Shaped<Record<string, DiffView>>
      /** The file `j` and `k` last moved to, by index into the selected diff; -1 at the top. */
      cursor: number
    }
  }
}
