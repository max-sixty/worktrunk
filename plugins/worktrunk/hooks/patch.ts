import type { FileDiff } from '../types'

/**
 * Splits a `git diff` patch into its files, keeping each file's hunks as the
 * source a `Code` element draws under `format: 'diff'`.
 *
 * Reads the headers git writes with `--no-color --no-ext-diff`, the `a/` and
 * `b/` prefixes and `core.quotePath=false`: `diff --git a/<old> b/<new>`, then
 * `new file`, `deleted file`, `rename to`, `Binary files` or `---`/`+++`
 * lines, then hunks. Git ends a `---`/`+++` name holding a space with a tab,
 * and still C-quotes one holding a tab, newline, quote or backslash.
 */
export function parsePatch(patch: string): FileDiff[] {
  const files: FileDiff[] = []
  for (const chunk of patch.split(/^(?=diff --git )/m)) {
    if (!chunk.startsWith('diff --git ')) {
      continue
    }
    const lines = chunk.split('\n')
    const first = lines[0] ?? ''
    const hunkAt = lines.findIndex(line => line.startsWith('@@'))
    const header = hunkAt === -1 ? lines : lines.slice(0, hunkAt)
    const body = hunkAt === -1 ? [] : lines.slice(hunkAt)
    const named = (prefix: string) => {
      const rest = header.find(line => line.startsWith(prefix))?.slice(prefix.length)
      return rest === undefined ? undefined : unquote(rest)
    }
    // A `---` or `+++` name without its side's prefix; `/dev/null` has none.
    const side = (prefix: string, at: string) => {
      const name = named(prefix)
      return name?.startsWith(at) === true ? name.slice(at.length) : undefined
    }
    const has = (prefix: string) => header.some(line => line.startsWith(prefix))

    // `+++ b/<path>` names the file after the change; a deletion has
    // `+++ /dev/null` and names it on `---`. A file with no hunks (binary,
    // mode-only, pure rename) names it only on `diff --git`.
    const quoted = / ("(?:[^"\\]|\\.)*")$/.exec(first)?.[1]
    const path =
      named('rename to ') ??
      side('+++ ', 'b/') ??
      side('--- ', 'a/') ??
      (quoted === undefined
        ? first.replace(/^diff --git a\/.* b\//, '')
        : unquote(quoted).replace(/^b\//, ''))

    let added = 0
    let deleted = 0
    for (const line of body) {
      if (line.startsWith('+')) {
        added += 1
      } else if (line.startsWith('-')) {
        deleted += 1
      }
    }

    const status: FileDiff['status'] = has('Binary files ')
      ? 'binary'
      : has('new file mode')
        ? 'added'
        : has('deleted file mode')
          ? 'deleted'
          : has('rename to ')
            ? 'renamed'
            : 'modified'

    const from = named('rename from ')
    const modes = [named('old mode '), named('new mode ')]
    const notes = [
      from === undefined ? null : `from ${from}`,
      modes[0] === undefined ? null : `mode ${modes[0]} → ${modes[1]}`,
    ].filter(note => note !== null)

    files.push({
      path: printable(path),
      status,
      added,
      deleted,
      note: notes.length === 0 ? null : printable(notes.join(', ')),
      hunks: printable(body.join('\n').replace(/\n+$/, '')),
    })
  }
  return files
}

const ESCAPES: Record<string, string> = {
  a: String.fromCharCode(7),
  b: '\b',
  f: '\f',
  n: '\n',
  r: '\r',
  t: '\t',
  v: '\v',
}

/**
 * A name as git wrote it on a header line: C-quoted and escaped, or bare and
 * ended with a tab when it holds a space.
 */
function unquote(name: string): string {
  if (!name.startsWith('"')) {
    return name.replace(/\t$/, '')
  }
  return name
    .slice(1, name.lastIndexOf('"'))
    .replace(/\\([0-7]{3}|.)/g, (_, code: string) =>
      code.length === 3 ? String.fromCharCode(Number.parseInt(code, 8)) : (ESCAPES[code] ?? code),
    )
}

/**
 * Replaces the control characters a drawn text refuses (all but tab and
 * newline: a CRLF file's `\r`, an escape sequence in a fixture) with U+FFFD.
 */
export function printable(text: string): string {
  return text.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, '�')
}
