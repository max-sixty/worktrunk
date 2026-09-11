// Sidebar order is authored here. Each page's `sidebar.order` frontmatter must
// agree with it: the docs sync test reads that value to order
// `docs/public/llms.txt`, and `test_sidebar_matches_frontmatter_order` fails
// when the two disagree.
export const sidebar = [
  { label: 'Overview', link: '/' },
  { label: 'Install', link: '/#install' },
  {
    label: 'Commands',
    items: [
      { label: 'wt switch', link: '/switch/' },
      { label: 'wt list', link: '/list/' },
      { label: 'wt merge', link: '/merge/' },
      { label: 'wt remove', link: '/remove/' },
      { label: 'wt config', link: '/config/' },
      { label: 'wt step', link: '/step/' },
      { label: 'wt hook', link: '/hook/' },
    ],
  },
  {
    label: 'Guides',
    items: [
      { label: 'Agent integration', link: '/claude-code/' },
      { label: 'Extending Worktrunk', link: '/extending/' },
      { label: 'LLM commit messages', link: '/llm-commits/' },
      { label: 'Tips & patterns', link: '/tips-patterns/' },
      { label: 'Shell integration', link: '/shell-integration/' },
      { label: 'FAQ', link: '/faq/' },
    ],
  },
];
