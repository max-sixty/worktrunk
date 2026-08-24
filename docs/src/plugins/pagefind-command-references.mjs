const fragmentTitleClass = 'wt-pagefind-fragment-title';

function nodeText(node) {
  if (node.type === 'text') return node.value;
  return node.children?.map(nodeText).join('') ?? '';
}

function isCommandHeading(text) {
  return /^wt\s+\S/u.test(text.trim());
}

function escapeHtml(text) {
  return text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
}

function annotateCommandReference(heading, command) {
  const alreadyAnnotated = heading.children?.some((child) => (
    child.type === 'raw' && child.value.includes(`class="${fragmentTitleClass}"`)
  ));
  if (alreadyAnnotated) return;

  heading.children ??= [];
  heading.children.unshift({
    // Starlight computes its anchor-link label before Astro parses raw HTML.
    // Keeping this as raw HTML prevents search context changing that label.
    type: 'raw',
    value: `<span class="${fragmentTitleClass}" hidden aria-hidden="true">${escapeHtml(command)} — </span>`,
  });
}

/**
 * Gives repeated command-reference fragments unique Pagefind titles.
 *
 * Pagefind derives sub-result titles from heading text and includes hidden HTML.
 * Browsers omit this hidden, aria-hidden prefix from rendering, selection, and the
 * accessibility tree, so the visible heading remains exactly "Command reference".
 */
export function rehypePagefindCommandReferences() {
  return (tree) => {
    let command;

    function visit(node) {
      if (node.type === 'element' && node.tagName === 'h2') {
        const text = nodeText(node).trim();
        command = isCommandHeading(text) ? text : undefined;
      } else if (
        command
        && node.type === 'element'
        && node.tagName === 'h3'
        && nodeText(node).trim() === 'Command reference'
      ) {
        annotateCommandReference(node, command);
      }
      node.children?.forEach(visit);
    }

    visit(tree);
  };
}
