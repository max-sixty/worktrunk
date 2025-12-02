# Docs Site

This is an initial scaffolding of the documentation site. The content was auto-generated from the main README and has not been reviewed for accuracy.

**Do not trust any content here.** Before this is production-ready:

- Survey all pages for accuracy
- Verify code examples work
- Check command output matches current behavior
- Review configuration options against actual implementation

This is being merged to enable iteration, not as finished documentation.

## Theme Architecture

The docs use a "warm workbench" theme built on top of the Juice Zola theme. Key files:

| File | Purpose |
|------|---------|
| `templates/_variables.html` | CSS custom properties (colors, layout, typography) |
| `sass/custom.scss` | All styling, organized by section |
| `templates/base.html` | Head overrides, IntersectionObserver disable |
| `templates/index.html` | Homepage hero and animations |
| `templates/page.html` | Doc page TOC rendering |

### Layout System

The sticky header and TOC use **definitional CSS variables** so positions are always in sync:

```
--wt-header-height: 60px    (includes border via box-sizing: border-box)
--wt-main-padding-top: 40px

TOC sticks at: calc(header + padding) = 100px
Anchor scroll-margin: same calculation
```

When either variable changes (including via media queries), all dependent values update automatically. This prevents the TOC from "jumping" when transitioning to sticky mode.

### Key Technical Decisions

1. **`box-sizing: border-box` on header** - Border is included in height, simplifying calculations
2. **`scrollbar-gutter: stable`** - Reserves scrollbar space to prevent layout shift on navigation
3. **IntersectionObserver intercept** - Disables Juice's scroll-spy which conflicts with our TOC styling
4. **Logo preload** - Prevents flash when navigating between pages
5. **WCAG AA colors** - `--wt-color-text-soft` is #78716a for 4.5:1 contrast

### Responsive Breakpoints

Variables are overridden in media queries to maintain definitional correctness:

- **≤1024px**: `--wt-main-padding-top: 30px`
- **≤768px**: `--wt-header-height: 50px`, `--wt-main-padding-top: 20px`, TOC hidden

### Extending the Theme

When adding new positioned elements:
- Use the layout variables rather than hardcoding pixel values
- Test anchor navigation to verify no visual jumps
- Check both with and without page scroll
