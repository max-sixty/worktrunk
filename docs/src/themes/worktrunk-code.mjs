const baseTokenColors = (palette) => [
  {
    settings: {
      background: palette.background,
      foreground: palette.foreground,
    },
  },
  {
    name: 'Comments',
    scope: ['comment', 'punctuation.definition.comment'],
    settings: {
      foreground: palette.comment,
      fontStyle: 'italic',
    },
  },
  {
    name: 'Keywords',
    scope: ['keyword', 'storage'],
    settings: {
      foreground: palette.keyword,
      fontStyle: 'bold',
    },
  },
  {
    name: 'Functions',
    scope: ['entity.name.function', 'meta.function-call', 'support.function.any-method'],
    settings: {
      foreground: palette.function,
      fontStyle: 'bold',
    },
  },
  {
    name: 'Strings',
    scope: ['string', 'constant.other.symbol', 'entity.other.inherited-class'],
    settings: { foreground: palette.string },
  },
  {
    name: 'Variables',
    scope: ['variable', 'support.class', 'entity.name.class', 'entity.name.type.class'],
    settings: { foreground: palette.variable },
  },
  {
    name: 'Constants',
    scope: ['constant', 'constant.numeric'],
    settings: { foreground: palette.constant },
  },
  {
    name: 'Support',
    scope: ['support.function', 'entity.name.tag', 'entity.other.attribute-name'],
    settings: { foreground: palette.support },
  },
  {
    name: 'Operators',
    scope: ['keyword.operator', 'punctuation.definition.variable', 'punctuation.definition.parameters'],
    settings: { foreground: palette.operator },
  },
  {
    name: 'Markup',
    scope: ['markup.heading', 'markup.raw.inline', 'markup.inserted'],
    settings: { foreground: palette.function },
  },
  {
    name: 'Invalid and deleted',
    scope: ['invalid', 'markup.deleted'],
    settings: { foreground: palette.invalid },
  },
];

function theme(name, type, palette) {
  return {
    name,
    type,
    colors: {
      'editor.background': palette.background,
      'editor.foreground': palette.foreground,
    },
    tokenColors: baseTokenColors(palette),
  };
}

// These are the syntax roles from the pre-Astro Worktrunk themes, adjusted only
// where the old foregrounds did not meet contrast on the current code surface.
export const worktrunkLightCodeTheme = theme('worktrunk-light', 'light', {
  background: '#f8f7f6',
  foreground: '#3d3632',
  comment: '#6d6258',
  keyword: '#9f4f2a',
  function: '#9b6500',
  string: '#527a16',
  variable: '#7e6247',
  constant: '#a03c15',
  support: '#9f4f2a',
  operator: '#3d3632',
  invalid: '#a03c15',
});

export const worktrunkDarkCodeTheme = theme('worktrunk-dark', 'dark', {
  background: '#252321',
  foreground: '#d8d4cf',
  comment: '#a8a29e',
  keyword: '#d8b4fe',
  function: '#e5b567',
  string: '#a3be8c',
  variable: '#d9a0a5',
  constant: '#e09a82',
  support: '#67d4d4',
  operator: '#d8d4cf',
  invalid: '#f87171',
});
