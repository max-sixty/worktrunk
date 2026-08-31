function nodeText(node) {
  if (node.type === 'text') return node.value;
  return node.children?.map(nodeText).join('') ?? '';
}

/** Preserve the heading-anchor scheme used by Worktrunk's original site. */
export function slugHeading(text) {
  return text
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
    .replace(/^-|-$/g, '');
}

export function rehypeStableHeadingIds() {
  return (tree) => {
    const slugs = new Map();

    function visit(node) {
      if (node.type === 'element' && /^h[1-6]$/.test(node.tagName)) {
        const base = slugHeading(nodeText(node));
        const occurrence = slugs.get(base) ?? 0;
        slugs.set(base, occurrence + 1);
        node.properties ??= {};
        node.properties.id = occurrence === 0 ? base : `${base}-${occurrence}`;
      }
      node.children?.forEach(visit);
    }

    visit(tree);
  };
}
