export const sidebar = [
  { label: 'Overview', link: '/' },
  {
    label: 'Commands',
    items: [
      { label: 'wt switch', link: '/switch/' },
      { label: 'wt list', link: '/list/' },
      { label: 'wt remove', link: '/remove/' },
      { label: 'wt merge', link: '/merge/' },
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
      { label: 'FAQ', link: '/faq/' },
    ],
  },
];
