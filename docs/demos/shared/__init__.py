"""Shared infrastructure for demo recording scripts."""

from .lib import (
    REAL_HOME,
    FIXTURES_DIR,
    VALIDATION_RS,
    DemoEnv,
    DemoSize,
    SIZE_SOCIAL,
    SIZE_DOCS,
    run,
    git,
    render_tape,
    record_vhs,
    build_wt,
    commit_dated,
    prepare_base_repo,
    setup_claude_code_config,
    setup_zellij_config,
    setup_fish_config,
    setup_mock_clis,
    prepare_demo_repo,
    # Demo recording infrastructure
    check_dependencies,
    setup_demo_output,
    record_all_themes,
    # Text output recording
    record_text,
    build_tape_replacements,
    # Snapshot recording
    extract_commands_from_tape,
    record_snapshot,
    # External dependencies
    ensure_vhs_binary,
)
from .themes import THEMES, format_theme_for_vhs

__all__ = [
    "REAL_HOME",
    "FIXTURES_DIR",
    "VALIDATION_RS",
    "DemoEnv",
    "DemoSize",
    "SIZE_SOCIAL",
    "SIZE_DOCS",
    "run",
    "git",
    "render_tape",
    "record_vhs",
    "build_wt",
    "commit_dated",
    "prepare_base_repo",
    "setup_claude_code_config",
    "setup_zellij_config",
    "setup_fish_config",
    "setup_mock_clis",
    "prepare_demo_repo",
    "THEMES",
    "format_theme_for_vhs",
    # Demo recording infrastructure
    "check_dependencies",
    "setup_demo_output",
    "record_all_themes",
    # Text output recording
    "record_text",
    "build_tape_replacements",
    # Snapshot recording
    "extract_commands_from_tape",
    "record_snapshot",
    # External dependencies
    "ensure_vhs_binary",
]
