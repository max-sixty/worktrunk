const recordClass = 'wt-responsive-records';
const labelClass = 'wt-responsive-record-label';

function elementChildren(node, tagName) {
  return (node.children ?? []).filter((child) => (
    child.type === 'element' && (!tagName || child.tagName === tagName)
  ));
}

function nodeText(node) {
  if (node.type === 'text') return node.value;
  return node.children?.map(nodeText).join('') ?? '';
}

function classNames(node) {
  const classes = node.properties?.className ?? [];
  return Array.isArray(classes) ? classes : String(classes).split(/\s+/u).filter(Boolean);
}

function hasComplexSpan(cell) {
  return ['colSpan', 'colspan', 'rowSpan', 'rowspan'].some((property) => (
    property in (cell.properties ?? {}) && Number(cell.properties[property]) !== 1
  ));
}

function recordTableParts(table) {
  if (classNames(table).includes('cmd-compare')) return;

  const sections = elementChildren(table);
  if (
    sections.length !== 2
    || sections[0].tagName !== 'thead'
    || sections[1].tagName !== 'tbody'
  ) return;

  const headerRows = elementChildren(sections[0], 'tr');
  const bodyRows = elementChildren(sections[1], 'tr');
  if (headerRows.length !== 1 || bodyRows.length < 1 || bodyRows.length > 3) return;

  const headers = elementChildren(headerRows[0]);
  if (
    headers.length < 3
    || headers.some((cell) => cell.tagName !== 'th' || hasComplexSpan(cell))
  ) return;

  const labels = headers.map((cell) => nodeText(cell).trim());
  if (labels.some((label) => !label) || new Set(labels).size !== labels.length) return;

  const rows = bodyRows.map((row) => elementChildren(row));
  if (rows.some((cells) => (
    cells.length !== headers.length
    || cells.some((cell) => cell.tagName !== 'td' || hasComplexSpan(cell))
  ))) return;

  return { labels, rows };
}

function annotateRecordTable(table) {
  const parts = recordTableParts(table);
  if (!parts) return;

  table.properties ??= {};
  const classes = classNames(table);
  if (!classes.includes(recordClass)) table.properties.className = [...classes, recordClass];
  table.properties.dataRecordColumns = String(parts.labels.length);

  for (const cells of parts.rows) {
    for (const [index, cell] of cells.entries()) {
      const alreadyLabeled = cell.children?.some((child) => (
        child.type === 'element' && classNames(child).includes(labelClass)
      ));
      if (alreadyLabeled) continue;
      cell.children ??= [];
      cell.children.unshift({
        type: 'element',
        tagName: 'span',
        properties: {
          ariaHidden: 'true',
          className: [labelClass],
        },
        children: [{ type: 'text', value: parts.labels[index] }],
      });
    }
  }
}

/**
 * Turns only short, unambiguous Markdown tables into responsive record groups.
 * Dense references and matrices retain their native horizontal table layout.
 */
export function rehypeResponsiveTables() {
  return (tree) => {
    function visit(node) {
      if (node.type === 'element' && node.tagName === 'table') annotateRecordTable(node);
      node.children?.forEach(visit);
    }

    visit(tree);
  };
}
