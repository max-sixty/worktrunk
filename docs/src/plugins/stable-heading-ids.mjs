export function nodeText(node) {
  if (node.type === 'text') return node.value;
  return node.children?.map(nodeText).join('') ?? '';
}

/** Whether a heading opens a generated subcommand section, such as "wt step commit". */
export function isCommandHeading(text) {
  return /^wt\s+\S/u.test(text.trim());
}

/** Preserve the heading-anchor scheme used by Worktrunk's original site. */
export function slugHeading(text) {
  return text
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
    .replace(/^-|-$/g, '');
}

/**
 * Gives every heading a public anchor id: its slug.
 *
 * Command pages append each subcommand's help under an H2 such as "wt step push",
 * so headings like "Examples" and "Command reference" repeat down the page. Below
 * a subcommand's H2, until the next H1 or H2, ids are scoped by that section:
 * "Examples" under "wt step push" is `wt-step-push--examples`. The id then depends
 * only on the heading and its subcommand, not on how many same-named headings
 * precede it, and the help text keeps its unqualified headings for the terminal.
 * A slug never contains `--`, so a scoped id cannot equal another heading's slug:
 * "Cache" under "wt config state" and the "wt config state cache" section differ.
 *
 * A heading that still repeats within one scope takes `-1`, `-2`, … in page order.
 */
export function rehypeStableHeadingIds() {
  return (tree) => {
    const slugs = new Map();
    let scope;

    function visit(node) {
      if (node.type === 'element' && /^h[1-6]$/.test(node.tagName)) {
        const text = nodeText(node);
        const level = Number(node.tagName[1]);
        if (level <= 2) scope = undefined;
        const base = scope ? `${scope}--${slugHeading(text)}` : slugHeading(text);
        const occurrence = slugs.get(base) ?? 0;
        slugs.set(base, occurrence + 1);
        node.properties ??= {};
        node.properties.id = occurrence === 0 ? base : `${base}-${occurrence}`;
        if (level === 2 && isCommandHeading(text)) scope = node.properties.id;
      }
      node.children?.forEach(visit);
    }

    visit(tree);
  };
}
