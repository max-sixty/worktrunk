"""Shared infrastructure for demo recording scripts."""

from .lib import (
    REAL_HOME,
    FIXTURES_DIR,
    DemoEnv,
    run,
    git,
    render_tape,
    record_vhs,
    build_wt,
    commit_dated,
    prepare_base_repo,
    setup_gh_mock,
    prepare_demo_repo,
    # Demo recording infrastructure
    check_dependencies,
    setup_demo_output,
    build_shell_env,
    clean_ansi_output,
    run_fish_script,
    record_all_themes,
)
from .themes import THEMES, format_theme_for_vhs

__all__ = [
    "REAL_HOME",
    "FIXTURES_DIR",
    "DemoEnv",
    "run",
    "git",
    "render_tape",
    "record_vhs",
    "build_wt",
    "commit_dated",
    "prepare_base_repo",
    "setup_gh_mock",
    "prepare_demo_repo",
    "THEMES",
    "format_theme_for_vhs",
    # Demo recording infrastructure
    "check_dependencies",
    "setup_demo_output",
    "build_shell_env",
    "clean_ansi_output",
    "run_fish_script",
    "record_all_themes",
]
