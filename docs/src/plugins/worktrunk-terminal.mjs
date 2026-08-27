import terminalStyles from '../generated/terminal-styles.json' with { type: 'json' };

const terminalBlocks = new WeakMap();
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

function removeTitlelessHeader(blockAst) {
  blockAst.children = blockAst.children.filter((child) => {
    if (
      child?.type !== 'element'
      || child.tagName !== 'figcaption'
      || !child.properties?.className?.includes('header')
    ) return true;

    const title = child.children?.find((element) => (
      element?.type === 'element'
      && element.properties?.className?.includes('title')
    ));
    return Boolean(
      title?.children?.some((element) => element.type !== 'text' || element.value.trim()),
    );
  });
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
  if (!segments.some(({ tone }) => tone)) return;
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

function shellCommentIndex(text) {
  let quote;
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (character === '\\' && quote !== "'") {
      escaped = true;
      continue;
    }
    if (character === quote) {
      quote = undefined;
      continue;
    }
    if (!quote && (character === "'" || character === '"')) {
      quote = character;
      continue;
    }
    if (!quote && character === '#' && (index === 0 || /\s/u.test(text[index - 1]))) {
      return index;
    }
  }
  return -1;
}

function renderCommand(lineAst, text) {
  const code = findCodeElement(lineAst);
  if (!code) return;
  const commentIndex = shellCommentIndex(text);
  const command = commentIndex === -1 ? text : text.slice(0, commentIndex);
  const comment = commentIndex === -1 ? '' : text.slice(commentIndex);
  code.children = [
    {
      type: 'element',
      tagName: 'span',
      properties: { className: ['wt-command-text'] },
      children: [{ type: 'text', value: command }],
    },
  ];
  if (comment) {
    code.children.push({
      type: 'element',
      tagName: 'span',
      properties: { className: ['wt-terminal-dim'] },
      children: [{ type: 'text', value: comment }],
    });
  }
}

/**
 * Keeps terminal examples as ordinary Markdown while removing untitled frame
 * chrome, adding command prompts, and copying commands rather than output.
 */
export function pluginWorktrunkTerminal() {
  return {
    name: 'Worktrunk terminal prompts',
    baseStyles: `
      .expressive-code .frame.is-terminal .ec-line.wt-command .code::before {
        content: '$ ';
        color: var(--wt-ink-muted);
        user-select: none;
      }
      .expressive-code .frame.is-terminal .wt-command-text {
        color: var(--wt-copper);
        font-weight: 550;
      }
      .expressive-code .frame.is-terminal .ec-line.wt-output .code {
        color: var(--sl-color-gray-2);
      }
      .expressive-code .frame.is-terminal .ec-line.wt-copyable .code {
        color: var(--sl-color-gray-3);
      }
      .expressive-code .frame.is-terminal .wt-positive {
        color: var(--sl-color-green-high);
        font-weight: 650;
      }
      .expressive-code .frame.is-terminal .wt-negative {
        color: var(--sl-color-red-high);
        font-weight: 650;
      }
      .expressive-code .frame.is-terminal .wt-warning {
        color: var(--sl-color-orange-high);
        font-weight: 650;
      }
      .expressive-code .frame.is-terminal .wt-terminal-red {
        color: var(--wt-terminal-red);
      }
      .expressive-code .frame.is-terminal .wt-terminal-green {
        color: var(--wt-terminal-green);
      }
      .expressive-code .frame.is-terminal .wt-terminal-yellow {
        color: var(--wt-terminal-yellow);
      }
      .expressive-code .frame.is-terminal .wt-terminal-blue {
        color: var(--wt-terminal-blue);
      }
      .expressive-code .frame.is-terminal .wt-terminal-magenta {
        color: var(--wt-terminal-magenta);
      }
      .expressive-code .frame.is-terminal .wt-terminal-cyan {
        color: var(--wt-terminal-cyan);
      }
      .expressive-code .frame.is-terminal .wt-terminal-gray {
        color: var(--wt-ink-muted);
      }
      .expressive-code .frame.is-terminal .wt-terminal-gutter {
        background: var(--wt-terminal-gutter);
      }
      .expressive-code .frame.is-terminal .wt-terminal-bold {
        font-weight: 600;
      }
      .expressive-code .frame.is-terminal .wt-terminal-dim {
        opacity: 0.9;
      }
      .expressive-code .frame.is-terminal .wt-terminal-italic {
        font-style: italic;
      }
      .expressive-code .frame.is-terminal .wt-terminal-underline {
        text-decoration: underline;
        text-underline-offset: 0.14em;
      }
    `,
    hooks: {
      preprocessCode({ codeBlock }) {
        if (codeBlock.language !== 'console') return;

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
          hasOutput,
          recordedByLine,
        });
      },
      postprocessRenderedLine({ codeBlock, line, lineIndex, renderData }) {
        const terminal = terminalBlocks.get(codeBlock);
        if (!terminal) return;
        const className = terminal.commandLines.has(lineIndex)
          ? 'wt-command'
          : terminal.copyableLines.has(lineIndex)
            ? 'wt-copyable'
            : 'wt-output';
        addClass(renderData.lineAst, className);
        const text = line?.text ?? codeBlock.getLines()[lineIndex].text;
        if (className === 'wt-command') {
          renderCommand(renderData.lineAst, text);
        } else if (className === 'wt-output') {
          const rendered = terminal.recordedByLine.has(lineIndex)
            && renderRecordedOutput(renderData.lineAst, terminal.recordedByLine.get(lineIndex));
          if (!rendered) renderSemanticOutput(renderData.lineAst, text);
        }
      },
      postprocessRenderedBlock({ codeBlock, renderData }) {
        removeTitlelessHeader(renderData.blockAst);
        const terminal = terminalBlocks.get(codeBlock);
        if (!terminal) {
          if (renderData.blockAst.properties?.className?.includes('is-terminal')) {
            addClass(renderData.blockAst, 'wt-commands-only');
          }
          return;
        }
        if (!terminal.hasOutput) addClass(renderData.blockAst, 'wt-commands-only');
        if (terminal.commandLines.size === 0 && terminal.copyableLines.size === 0) {
          removeCopyControl(renderData.blockAst);
          return;
        }
        const copyText = [...codeBlock.getLines()]
          .filter((_, lineIndex) => (
            terminal.commandLines.has(lineIndex) || terminal.copyableLines.has(lineIndex)
          ))
          .map((line) => line.text)
          .join('\u007f');
        setCopyText(renderData.blockAst, copyText);
      },
    },
  };
}
