import { fromHtml } from 'hast-util-from-html';

import terminalStyles from '../generated/terminal-styles.json' with { type: 'json' };

const terminalBlocks = new WeakMap();
const consoleBlocks = new WeakSet();
const commandReferenceBlocks = new WeakSet();
const commandReferenceMeta = 'wt-command-reference';
const recordedTerminalBlocks = new Map(
  terminalStyles.map(({ plain, lines }) => [plain, lines]),
);

const semanticMarker = /((?:(?<=^)|(?<=\s{2}))[-+↑⇡↓⇣]\d+(?=\s{2}|$)|✓|✗|(?<=^|\s)[!?](?=\s|$))/gmu;

function addClass(node, className) {
  node.properties ??= {};
  const classes = node.properties.className ?? [];
  node.properties.className = Array.isArray(classes)
    ? [...classes, className]
    : [classes, className];
}

function classNames(node) {
  const classes = node.properties?.className ?? [];
  return Array.isArray(classes) ? classes : String(classes).split(/\s+/u).filter(Boolean);
}

function nodeText(node) {
  if (node.type === 'text') return node.value;
  return node.children?.map(nodeText).join('') ?? '';
}

function findCodeElement(node) {
  if (
    node?.type === 'element'
    && node.tagName === 'div'
    && node.properties?.className?.includes('code')
  ) return node;
  for (const child of node?.children ?? []) {
    const code = findCodeElement(child);
    if (code) return code;
  }
}

function removeCopyControl(blockAst) {
  blockAst.children = blockAst.children.filter((child) => (
    child?.type !== 'element'
    || !child.properties?.className?.includes('copy')
  ));
}

function setCopyText(blockAst, copyText) {
  const copy = blockAst.children.find((child) => (
    child?.type === 'element' && child.properties?.className?.includes('copy')
  ));
  const button = copy?.children?.find((child) => (
    child?.type === 'element' && child.tagName === 'button'
  ));
  if (!button) return;
  button.properties ??= {};
  button.properties['data-code'] = copyText;
}

/// Expressive Code's copy script binds every `.expressive-code .copy button`
/// and reads `data-code` (newlines encoded as U+007F), so a control built to
/// the same shape works wherever it is placed inside the frame.
function copyControl(text, className) {
  return {
    type: 'element',
    tagName: 'div',
    properties: { className: ['copy', className] },
    children: [
      { type: 'element', tagName: 'div', properties: { 'aria-live': 'polite' }, children: [] },
      {
        type: 'element',
        tagName: 'button',
        properties: { title: 'Copy this line', 'data-copied': 'Copied!', 'data-code': text },
        children: [{ type: 'element', tagName: 'div', properties: {}, children: [] }],
      },
    ],
  };
}

function markerTone(marker) {
  if (/^[+↑⇡✓]/u.test(marker)) return 'positive';
  if (/^[-↓⇣✗]/u.test(marker)) return 'negative';
  return 'warning';
}

/** Split captured output into neutral text and semantically colored markers. */
export function semanticOutputSegments(text) {
  const segments = [];
  let cursor = 0;
  for (const match of text.matchAll(semanticMarker)) {
    if (match.index > cursor) segments.push({ text: text.slice(cursor, match.index) });
    segments.push({ text: match[0], tone: markerTone(match[0]) });
    cursor = match.index + match[0].length;
  }
  if (cursor < text.length) segments.push({ text: text.slice(cursor) });
  return segments;
}

function renderSemanticOutput(lineAst, text) {
  const segments = semanticOutputSegments(text);
  const code = findCodeElement(lineAst);
  if (!code) return;
  code.children = segments.map(({ text: value, tone }) => tone
    ? {
        type: 'element',
        tagName: 'span',
        properties: { className: [`wt-${tone}`] },
        children: [{ type: 'text', value }],
      }
      : { type: 'text', value });
}

function renderRecordedOutput(lineAst, segments) {
  const code = findCodeElement(lineAst);
  if (!code) return false;
  code.children = segments.map(({ text: value, classes }) => classes.length > 0
    ? {
        type: 'element',
        tagName: 'span',
        properties: { className: classes },
        children: [{ type: 'text', value }],
      }
    : { type: 'text', value });
  return true;
}

function appendSegment(segments, text, tone) {
  if (!text) return;
  const previous = segments.at(-1);
  if (previous && previous.tone === tone) {
    previous.text += text;
  } else {
    segments.push(tone ? { text, tone } : { text });
  }
}

/** Split a shell recipe into the syntax roles used by the site code theme. */
export function shellCommandSegments(text) {
  const segments = [];
  const tokens = /(?<space>\s+)|(?<operator>&&|\\)|(?<word>[^\s&\\]+)/gu;
  let cursor = 0;
  let expectCommand = true;

  for (const match of text.matchAll(tokens)) {
    appendSegment(segments, text.slice(cursor, match.index));
    const token = match[0];
    if (match.groups.space) {
      appendSegment(segments, token);
      if (token.includes('\n')) expectCommand = true;
      cursor = match.index + token.length;
      continue;
    }
    if (match.groups.operator) {
      appendSegment(segments, token);
      if (token === '&&') expectCommand = true;
      cursor = match.index + token.length;
      continue;
    }

    const tone = expectCommand
      ? 'command'
      : /^--?[A-Za-z0-9]/u.test(token)
        ? 'option'
        : 'argument';
    appendSegment(segments, token, tone);
    expectCommand = false;
    cursor = match.index + token.length;
  }
  appendSegment(segments, text.slice(cursor));

  return segments;
}

/**
 * A `#` that begins an unquoted word starts a shell comment. Zsh passes one to
 * the command as arguments unless `interactivecomments` is set, so copied
 * commands leave trailing comments out. A line that is only a comment stays.
 * Heredocs, backticks, and `$'…'` strings aren't parsed; no example uses them.
 */
export function withoutTrailingShellComments(code) {
  let output = '';
  let quote;
  let wordStart = true;
  for (let index = 0; index < code.length; index += 1) {
    const char = code[index];
    if (quote) {
      output += char;
      if (char === quote) quote = undefined;
      else if (char === '\\' && quote === '"') output += code[++index] ?? '';
      wordStart = false;
    } else if (char === '\\') {
      output += char + (code[++index] ?? '');
      wordStart = false;
    } else if (char === "'" || char === '"') {
      output += char;
      quote = char;
      wordStart = false;
    } else if (char === '#' && wordStart) {
      const newline = code.indexOf('\n', index);
      const lineEnd = newline === -1 ? code.length : newline;
      const lineSoFar = output.slice(output.lastIndexOf('\n') + 1);
      output = lineSoFar.trim() === '' ? output + code.slice(index, lineEnd) : output.trimEnd();
      index = lineEnd - 1;
    } else {
      output += char;
      wordStart = /[\s;&|()<>]/u.test(char);
    }
  }
  return output;
}

function renderShellCommand(code) {
  code.children = shellCommandSegments(nodeText(code)).map(({ text: value, tone }) => tone
    ? {
        type: 'element',
        tagName: 'span',
        properties: { className: [`wt-shell-${tone}`] },
        children: [{ type: 'text', value }],
      }
    : { type: 'text', value });
}

/** Add build-only shell syntax roles to the homepage command comparison. */
export function rehypeComparisonCommands() {
  return (tree) => {
    function decorate(node, insideComparison = false) {
      const comparison = insideComparison || (
        node.type === 'element'
        && node.tagName === 'table'
        && classNames(node).includes('cmd-compare')
      );
      if (comparison && node.type === 'element' && node.tagName === 'code') {
        renderShellCommand(node);
        return;
      }
      node.children?.forEach((child) => decorate(child, comparison));
    }

    function expandRawComparisons(node) {
      for (let index = 0; index < (node.children?.length ?? 0); index += 1) {
        const child = node.children[index];
        if (child.type === 'raw' && child.value.includes('cmd-compare')) {
          const fragment = fromHtml(child.value, { fragment: true });
          const comparison = fragment.children.find((candidate) => (
            candidate.type === 'element'
            && candidate.tagName === 'table'
            && classNames(candidate).includes('cmd-compare')
          ));
          if (comparison) {
            decorate(comparison);
            node.children.splice(index, 1, ...fragment.children);
            index += fragment.children.length - 1;
            continue;
          }
        }
        expandRawComparisons(child);
      }
    }

    decorate(tree);
    expandRawComparisons(tree);
  };
}

function helpInlineSegments(text) {
  const segments = [];
  const tokenPattern = /\[[^\]\n]+:\s*[^\]\n]+\]|\[experimental\]|--[a-z0-9][a-z0-9-]*(?:\.\.\.)?|(?<![\w-])-[A-Za-z](?=[,\s]|$)|<[^>\n]+>(?:\.\.\.)?|\[[A-Z][A-Z0-9_-]*\](?:\.\.\.)?/gu;
  let cursor = 0;
  for (const match of text.matchAll(tokenPattern)) {
    appendSegment(segments, text.slice(cursor, match.index));
    const token = match[0];
    const tone = token.startsWith('-')
      ? 'option'
      : token === '[experimental]' || (token.startsWith('[') && token.includes(':'))
        ? 'meta'
        : 'value';
    appendSegment(segments, token, tone);
    cursor = match.index + token.length;
  }
  appendSegment(segments, text.slice(cursor));
  return segments;
}

/** Parse the stable clap help grammar into semantic syntax roles. */
export function commandReferenceSegments(text) {
  const commandHeader = text.match(/^(wt(?: [a-z][\w-]*)*) - (.+)$/u);
  if (commandHeader) {
    return [
      { text: commandHeader[1], tone: 'command' },
      { text: ' - ' },
      ...helpInlineSegments(commandHeader[2]),
    ];
  }

  const sectionHeading = text.match(/^([A-Z][A-Za-z ]+):$/u);
  if (sectionHeading) return [{ text, tone: 'heading' }];

  const nestedHeading = text.match(/^(\s+)([A-Z][A-Za-z ]+:)$/u);
  if (nestedHeading) {
    return [
      { text: nestedHeading[1] },
      { text: nestedHeading[2], tone: 'meta' },
    ];
  }

  const possibleValue = text.match(/^(\s+- )([^:\s]+)(?::(.*))?$/u);
  if (possibleValue) {
    return [
      { text: possibleValue[1] },
      { text: possibleValue[2], tone: 'value' },
      ...(possibleValue[3] === undefined
        ? []
        : [{ text: ':' }, ...helpInlineSegments(possibleValue[3])]),
    ];
  }

  const usage = text.match(/^(Usage:)(\s+)(wt(?: [a-z][\w-]*)*)(.*)$/u);
  if (usage) {
    return [
      { text: usage[1], tone: 'heading' },
      { text: usage[2] },
      { text: usage[3], tone: 'command' },
      ...helpInlineSegments(usage[4]),
    ];
  }

  const usageContinuation = text.match(/^(\s+)(wt(?: [a-z][\w-]*)*)(\s+(?:\[|<).*)$/u);
  if (usageContinuation) {
    return [
      { text: usageContinuation[1] },
      { text: usageContinuation[2], tone: 'command' },
      ...helpInlineSegments(usageContinuation[3]),
    ];
  }

  const subcommand = text.match(/^(\s+)([a-z][\w-]*)(\s{2,})(.+)$/u);
  if (subcommand) {
    return [
      { text: subcommand[1] },
      { text: subcommand[2], tone: 'command' },
      { text: subcommand[3] },
      ...helpInlineSegments(subcommand[4]),
    ];
  }

  return helpInlineSegments(text);
}

function renderCommandReference(lineAst, text) {
  const code = findCodeElement(lineAst);
  if (!code) return;
  const leadingSpaces = text.match(/^ */u)?.[0].length ?? 0;
  if (leadingSpaces > 0) {
    const indentLevel = leadingSpaces <= 2 ? 1 : leadingSpaces <= 7 ? 2 : 3;
    addClass(lineAst, `wt-help-indent-${indentLevel}`);
  }
  const segments = commandReferenceSegments(text);
  if (leadingSpaces > 0 && segments[0]) {
    segments[0].text = segments[0].text.slice(leadingSpaces);
  }
  code.children = segments.map(({ text: value, tone }) => tone
    ? {
        type: 'element',
        tagName: 'span',
        properties: { className: [`wt-help-${tone}`] },
        children: [{ type: 'text', value }],
      }
    : { type: 'text', value });
}

/**
 * Keeps terminal examples as ordinary Markdown while adding command prompts
 * and copying commands rather than output.
 */
export function pluginWorktrunkTerminal() {
  return {
    name: 'Worktrunk terminal prompts',
    baseStyles: `
      .expressive-code .frame .ec-line.wt-command .code::before {
        content: '$ ';
        color: var(--wt-ink-muted);
        user-select: none;
      }
      .expressive-code .frame .ec-line.wt-output .code {
        color: var(--wt-terminal-ink);
      }
      .expressive-code .frame .ec-line.wt-copyable .code {
        color: var(--wt-terminal-dim);
      }
      .expressive-code .frame .wt-positive {
        color: var(--sl-color-green-high);
        font-weight: 650;
      }
      .expressive-code .frame .wt-negative {
        color: var(--sl-color-red-high);
        font-weight: 650;
      }
      .expressive-code .frame .wt-warning {
        color: var(--sl-color-orange-high);
        font-weight: 650;
      }
      .expressive-code .frame .wt-terminal-red {
        color: var(--wt-terminal-red);
      }
      .expressive-code .frame .wt-terminal-green {
        color: var(--wt-terminal-green);
      }
      .expressive-code .frame .wt-terminal-yellow {
        color: var(--wt-terminal-yellow);
      }
      .expressive-code .frame .wt-terminal-blue {
        color: var(--wt-terminal-blue);
      }
      .expressive-code .frame .wt-terminal-magenta {
        color: var(--wt-terminal-magenta);
      }
      .expressive-code .frame .wt-terminal-cyan {
        color: var(--wt-terminal-cyan);
      }
      .expressive-code .frame .wt-terminal-gray {
        color: var(--wt-ink-muted);
      }
      .expressive-code .frame .wt-terminal-gutter {
        display: inline-block;
        background: var(--wt-terminal-gutter);
      }
      .expressive-code .frame .wt-terminal-bold {
        font-weight: 600;
      }
      .expressive-code .frame .wt-terminal-dim {
        color: var(--wt-terminal-dim);
        opacity: 1;
      }
      .expressive-code .frame .wt-terminal-red.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-red) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-green.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-green) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-yellow.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-yellow) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-blue.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-blue) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-magenta.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-magenta) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-cyan.wt-terminal-dim {
        color: color-mix(in srgb, var(--wt-terminal-cyan) 62%, var(--wt-terminal-dim));
      }
      .expressive-code .frame .wt-terminal-italic {
        font-style: italic;
      }
      .expressive-code .frame .wt-terminal-underline {
        text-decoration: underline;
        text-underline-offset: 0.14em;
      }
      .expressive-code .frame.wt-line-copies .ec-line.wt-command {
        position: relative;
      }
      .expressive-code .frame.wt-line-copies .ec-line.wt-command .code {
        padding-inline-end: calc(2rem + var(--ec-codePadInl));
      }
      /* A line's control never outgrows its line, so adjacent controls can't overlap. */
      .expressive-code .frame .wt-line-copy {
        inset-block: 0;
      }
      .expressive-code .frame .wt-line-copy button {
        align-self: flex-start;
        max-height: 100%;
      }
      /* Expressive Code sizes the icon from the button's height; keep its desktop size. */
      .expressive-code .frame .wt-line-copy button::after {
        margin: 0;
        -webkit-mask-position: center;
        mask-position: center;
        -webkit-mask-size: 1.05rem;
        mask-size: 1.05rem;
      }
      @media (hover: hover) {
        .expressive-code .frame:hover .wt-line-copy button:not(:hover) {
          opacity: 0;
        }
        .expressive-code .frame .ec-line:hover .wt-line-copy button:not(:hover) {
          opacity: 0.75;
        }
      }
      .expressive-code .frame.wt-command-reference .wt-help-heading {
        color: var(--wt-copper);
        font-weight: 650;
      }
      .expressive-code .frame.wt-command-reference .wt-help-command {
        color: var(--wt-code-command);
        font-weight: 600;
      }
      .expressive-code .frame.wt-command-reference .wt-help-option {
        color: var(--wt-code-option);
      }
      .expressive-code .frame.wt-command-reference .wt-help-value {
        color: var(--wt-code-string);
      }
      .expressive-code .frame.wt-command-reference .wt-help-meta {
        color: var(--wt-terminal-dim);
        font-style: italic;
      }
      .expressive-code .frame.wt-command-reference .wt-help-indent-1 .code {
        padding-inline-start: calc(var(--ec-codePadInl) + 2ch);
      }
      .expressive-code .frame.wt-command-reference .wt-help-indent-2 .code {
        padding-inline-start: calc(var(--ec-codePadInl) + 6ch);
      }
      .expressive-code .frame.wt-command-reference .wt-help-indent-3 .code {
        padding-inline-start: calc(var(--ec-codePadInl) + 9ch);
      }
      @media (max-width: 42rem) {
        .expressive-code .frame.wt-command-reference .wt-help-indent-1 .code {
          padding-inline-start: calc(var(--ec-codePadInl) + 1ch);
        }
        .expressive-code .frame.wt-command-reference .wt-help-indent-2 .code {
          padding-inline-start: calc(var(--ec-codePadInl) + 2ch);
        }
        .expressive-code .frame.wt-command-reference .wt-help-indent-3 .code {
          padding-inline-start: calc(var(--ec-codePadInl) + 3ch);
        }
      }
    `,
    hooks: {
      preprocessLanguage({ codeBlock }) {
        if (codeBlock.language === 'console') {
          consoleBlocks.add(codeBlock);
          codeBlock.language = 'bash';
          return;
        }
        if (codeBlock.metaOptions.value(commandReferenceMeta) === true) {
          commandReferenceBlocks.add(codeBlock);
        }
      },
      preprocessCode({ codeBlock }) {
        if (!consoleBlocks.has(codeBlock)) return;

        const lines = [...codeBlock.getLines()];
        const commandLines = new Set();
        const copyableLines = new Set();
        const hasOutput = lines.some((line) => (
          line.text !== '' && !line.text.startsWith('$ ') && !line.text.startsWith('#')
        ));
        for (const [lineIndex, line] of lines.entries()) {
          if (line.text.startsWith('$ ')) {
            commandLines.add(lineIndex);
            line.editText(0, 2, '');
          } else if (line.text.startsWith('#') || (line.text === '' && !hasOutput)) {
            copyableLines.add(lineIndex);
          }
        }
        // Per-line copy controls sit at the end of their line, so a block with
        // several commands wraps rather than scrolling them out of view. Captured
        // output keeps its columns, so a block carrying it never wraps.
        if (commandLines.size > 1 && !hasOutput) codeBlock.props.wrap = true;
        const outputLines = lines
          .map((line, lineIndex) => ({ line, lineIndex }))
          .filter(({ lineIndex }) => (
            !commandLines.has(lineIndex) && !copyableLines.has(lineIndex)
          ));
        const outputText = outputLines.map(({ line }) => line.text).join('\n').trimEnd();
        const recordedLines = recordedTerminalBlocks.get(outputText);
        const recordedByLine = recordedLines?.length === outputLines.length
          ? new Map(outputLines.map(({ lineIndex }, index) => [lineIndex, recordedLines[index]]))
          : new Map();
        terminalBlocks.set(codeBlock, {
          commandLines,
          copyableLines,
          recordedByLine,
        });
      },
      postprocessRenderedLine({ codeBlock, line, lineIndex, renderData }) {
        if (commandReferenceBlocks.has(codeBlock)) {
          addClass(renderData.lineAst, 'wt-help-line');
          renderCommandReference(renderData.lineAst, line?.text ?? '');
          return;
        }
        const terminal = terminalBlocks.get(codeBlock);
        if (!terminal) return;
        const className = terminal.commandLines.has(lineIndex)
          ? 'wt-command'
          : terminal.copyableLines.has(lineIndex)
            ? 'wt-copyable'
            : 'wt-output';
        addClass(renderData.lineAst, className);
        const text = line?.text ?? codeBlock.getLines()[lineIndex].text;
        // Most blocks listing several commands are menus of alternatives, so
        // each command line gets its own control in place of the block's.
        if (className === 'wt-command' && terminal.commandLines.size > 1) {
          renderData.lineAst.children ??= [];
          renderData.lineAst.children.push(
            copyControl(withoutTrailingShellComments(text), 'wt-line-copy'),
          );
        }
        if (className === 'wt-output') {
          const rendered = terminal.recordedByLine.has(lineIndex)
            && renderRecordedOutput(renderData.lineAst, terminal.recordedByLine.get(lineIndex));
          if (!rendered) renderSemanticOutput(renderData.lineAst, text);
        }
      },
      postprocessRenderedBlock({ codeBlock, renderData }) {
        if (commandReferenceBlocks.has(codeBlock)) {
          addClass(renderData.blockAst, 'wt-command-reference');
          // Generated `--help` output is reference material — its copy button
          // offered thousands of characters nobody pastes anywhere.
          removeCopyControl(renderData.blockAst);
          return;
        }
        const terminal = terminalBlocks.get(codeBlock);
        if (!terminal) {
          if (codeBlock.language === 'bash' || codeBlock.language === 'sh') {
            setCopyText(renderData.blockAst, withoutTrailingShellComments(codeBlock.code).replaceAll('\n', '\u007f'));
          }
          return;
        }
        if (terminal.commandLines.size > 1) {
          addClass(renderData.blockAst, 'wt-line-copies');
          removeCopyControl(renderData.blockAst);
          return;
        }
        if (terminal.commandLines.size === 0 && terminal.copyableLines.size === 0) {
          removeCopyControl(renderData.blockAst);
          return;
        }
        const copyText = [...codeBlock.getLines()]
          .flatMap((line, lineIndex) => {
            if (terminal.commandLines.has(lineIndex)) return [withoutTrailingShellComments(line.text)];
            return terminal.copyableLines.has(lineIndex) ? [line.text] : [];
          })
          .join('\u007f');
        setCopyText(renderData.blockAst, copyText);
      },
    },
  };
}
