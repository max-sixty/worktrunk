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
]
