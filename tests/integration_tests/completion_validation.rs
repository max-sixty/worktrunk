use crate::common::wt_command;
use std::collections::HashSet;

/// Drive the dynamic completion engine for a command-line context and return
/// the set of candidate values it offers.
///
/// A shell requests completions by invoking `COMPLETE=<shell> wt -- <words…>`,
/// where the final (possibly empty) word is the token under the cursor. Worktrunk
/// answers on stdout: one candidate per line, `value<TAB>help` for shells that
/// render descriptions (fish/zsh). We key on `fish` and keep the value before the
/// first tab.
fn completion_candidates(words: &[&str]) -> HashSet<String> {
    let mut cmd = wt_command();
    cmd.env("COMPLETE", "fish");
    cmd.arg("--");
    cmd.args(words);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "completion invocation for {words:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split('\t').next())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// `hide = true` subcommands must never appear in completions.
///
/// Worktrunk generates completions dynamically (clap_complete's completion
/// engine, driven from `COMPLETE=<shell> wt`), assembled in
/// `src/completion.rs`. Deprecated and internal subcommands are marked
/// `#[command(hide = true)]` so they stay out of `--help` *and* out of
/// completions; a regression that resurfaced one (e.g. an injection layer
/// re-adding it, or a new internal subcommand missing `hide = true`) would leak
/// it to every user's shell.
///
/// Each context also asserts a visible sibling is present. That guard is
/// load-bearing: without it, a broken invocation returning *no* candidates would
/// satisfy every "hidden subcommand absent" check vacuously — the exact failure mode
/// that let this test rot after completions moved from static generation
/// (`wt config shell init`, which no longer emits any flag lines) to the dynamic
/// engine.
///
/// Hidden *flags* are deliberately not checked here: unlike subcommands, the
/// completion assembly (`hide_non_positional_options_for_completion`) surfaces
/// every long flag — hidden ones included — when the user explicitly types `--`,
/// so "hidden flags never appear" is not an invariant the current design holds.
#[test]
fn test_hidden_subcommands_excluded_from_completions() {
    // `wt <Tab>` — the legacy `select` subcommand is hidden (superseded by the
    // picker integrated into `wt switch`).
    let top_level = completion_candidates(&["wt", ""]);
    assert!(
        top_level.contains("switch"),
        "expected visible `switch` in top-level completions: {top_level:?}"
    );
    assert!(
        !top_level.contains("select"),
        "hidden `select` leaked into `wt` completions: {top_level:?}"
    );

    // `wt hook <Tab>` — the internal `run-pipeline` runner and the deprecated
    // `approvals` alias are hidden; the hook types are offered.
    let hook = completion_candidates(&["wt", "hook", ""]);
    assert!(
        hook.contains("pre-merge"),
        "expected visible `pre-merge` in hook completions: {hook:?}"
    );
    for hidden in ["run-pipeline", "approvals"] {
        assert!(
            !hook.contains(hidden),
            "hidden `hook {hidden}` leaked into completions: {hook:?}"
        );
    }

    // `wt config shell <Tab>` — the internal `completions` generator (emits the
    // package-manager completion registration) is hidden; the interactive setup
    // subcommands are offered. `completions` lives here, not directly under
    // `wt config`, so it must be probed at this level to actually exercise the
    // hide filtering rather than the tree structure.
    let config_shell = completion_candidates(&["wt", "config", "shell", ""]);
    assert!(
        config_shell.contains("install"),
        "expected visible `install` in config shell completions: {config_shell:?}"
    );
    assert!(
        !config_shell.contains("completions"),
        "hidden `config shell completions` leaked into completions: {config_shell:?}"
    );

    // `wt config state <Tab>` — the deprecated per-category subcommands (now
    // folded into `wt config state cache`) are hidden; `cache` is offered. Like
    // `completions` above, these live under `state`, not directly under
    // `wt config`, so they must be probed at this level.
    let config_state = completion_candidates(&["wt", "config", "state", ""]);
    assert!(
        config_state.contains("cache"),
        "expected visible `cache` in config state completions: {config_state:?}"
    );
    for hidden in ["previous-branch", "hints", "ci-status"] {
        assert!(
            !config_state.contains(hidden),
            "hidden `config state {hidden}` leaked into completions: {config_state:?}"
        );
    }
}
