//! Shared documentation markers.
//!
//! The help-page generator and documentation sync tests both use these
//! constants. Keeping the producer and consumer on one definition makes
//! marker drift a compile-time failure.

/// Empty span; the displayed "EXPERIMENTAL" text comes from the CSS `::after`
/// rule so generated anchor slugs ignore it.
///
/// Lives here (rather than in `src/help.rs`) so the producer (the `--help-page`
/// post-processor that emits this in place of `[experimental]`) and the
/// consumer (the skill-file generator that strips it back to `[experimental]`)
/// stay in lockstep — changing the badge in one place would otherwise let
/// stale HTML leak silently into skill files.
pub const BADGE_EXPERIMENTAL_HTML: &str = "<span class=\"badge-experimental\"></span>";

/// HTML-comment marker that includes a subcommand's `--help-page` body inline
/// in its parent's `after_long_help`. Expanded into an H2 section by
/// `expand_subdoc_placeholders`. Trailing space included so `find()` and
/// `strip_prefix()` agree on the boundary.
pub const SUBDOC_MARKER_PREFIX: &str = "<!-- subdoc: ";

/// HTML-comment marker that pulls a demo GIF from `worktrunk-assets` into the
/// docs site (rendered as a `<picture>` figure). Stripped from skill output.
pub const DEMO_MARKER_PREFIX: &str = "<!-- demo: ";

/// Open-marker prefix for an auto-generated region in a docs file.
/// Followed by `<id> — edit <source> to update -->`. Both `--help-page` (the
/// producer in `src/help.rs`) and the doc-sync test (the consumer in
/// `tests/integration_tests/readme_sync.rs`) reference this so the literal
/// can't drift between sides.
pub const MARKER_OPEN_PREFIX: &str = "<!-- ⚠️ AUTO-GENERATED from ";

/// Close marker for an auto-generated region. Paired with `MARKER_OPEN_PREFIX`
/// via non-greedy regex matching; the sync test
/// `test_no_nested_auto_generated_markers` enforces the no-nesting precondition
/// that makes that pairing safe.
pub const MARKER_CLOSE: &str = "<!-- END AUTO-GENERATED -->";
