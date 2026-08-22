const terminalBlocks = new WeakMap();

// Bare `+` is a status only at a line edge or between wide table columns;
// ordinary prose such as `[unoptimized + debuginfo]` stays neutral.
const semanticMarker = /([+↑⇡]\d+|[↑⇡]|(?<!\s)\+|\+(?!\s)|(?<=^|\s\s)\+(?=\s\s|\s*$)|[-↓⇣]\d+|✓|✗|(?<=^|\s)[!?](?=\s|$))/gmu;

function addClass(node, className) {
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
        color: var(--sl-color-accent-high);
        font-weight: 650;
        user-select: none;
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
        terminalBlocks.set(codeBlock, { commandLines, copyableLines });
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
        if (className === 'wt-output') {
          renderSemanticOutput(renderData.lineAst, line?.text ?? codeBlock.getLines()[lineIndex].text);
        }
      },
      postprocessRenderedBlock({ codeBlock, renderData }) {
        removeTitlelessHeader(renderData.blockAst);
        const terminal = terminalBlocks.get(codeBlock);
        if (!terminal) return;
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
