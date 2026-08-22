import runpy
import sys
from pathlib import Path

DEMOS_DIR = Path(__file__).parents[1]
REPO_ROOT = DEMOS_DIR.parents[1]
sys.path.insert(0, str(DEMOS_DIR))

from shared import (  # noqa: E402
    DemoSize,
    extract_commands_from_tape,
    render_tape,
)


def test_each_theme_records_from_a_fresh_environment(tmp_path: Path) -> None:
    build = runpy.run_path(str(DEMOS_DIR / "build"))
    fake_vhs = tmp_path / "fake-vhs"
    fake_vhs.write_text(
        """#!/usr/bin/env python3
import re
import sys
from pathlib import Path

tape = Path(sys.argv[1]).read_text()
output = Path(re.search(r'^Output "([^"]+)"', tape, re.MULTILINE).group(1))
output.parent.mkdir(parents=True, exist_ok=True)
output.write_bytes(b'GIF89a')
Path(f'{output}.tape').write_text(tape)
"""
    )
    fake_vhs.chmod(0o755)

    tape = tmp_path / "demo.tape"
    tape.write_text(
        """Set Width {{WIDTH}}
Set Height {{HEIGHT}}
Set FontSize {{FONTSIZE}}
Set Theme {{THEME}}
Output "{{OUTPUT_GIF}}"
Env HOME "{{DEMO_HOME}}"
"""
    )
    output_gifs = {
        theme: tmp_path / theme / "recording.gif" for theme in ("light", "dark")
    }
    environments: list[Path] = []

    def setup_environment(environment) -> None:
        environments.append(environment.home.resolve())

    build["record_demo"].__globals__["TAPES_DIR"] = tmp_path
    build["record_demo"](
        tape.name,
        "recording",
        setup_environment,
        tmp_path,
        list(output_gifs),
        DemoSize(width=1600, height=900, fontsize=24),
        vhs_binary=str(fake_vhs),
    )

    assert len(environments) == 2
    light_tape = Path(f"{output_gifs['light']}.tape").read_text()
    dark_tape = Path(f"{output_gifs['dark']}.tape").read_text()
    assert str(environments[0]) in light_tape
    assert str(environments[1]) not in light_tape
    assert str(environments[1]) in dark_tape
    assert str(environments[0]) not in dark_tape


def test_docs_demos_open_on_a_complete_command_before_execution() -> None:
    build = runpy.run_path(str(DEMOS_DIR / "build"))
    first_commands = {
        "wt-core.tape": "wt list",
        "wt-core-mobile.tape": "wt list",
        "wt-switch.tape": "wt switch alpha",
        "wt-list.tape": "wt list",
        "wt-commit.tape": "wt switch hooks",
        "wt-statusline.tape": "wt switch alpha",
        "wt-merge.tape": "wt list",
        "wt-switch-picker.tape": "wt switch",
        "wt-zellij-omnibus.tape": "wt switch",
    }

    assert {tape for tape, _, _ in build["DOCS_DEMOS"]} == set(first_commands)
    for tape_file, expected_command in first_commands.items():
        tape = DEMOS_DIR / "tapes" / tape_file
        lines = [
            line.strip()
            for line in tape.read_text().splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]
        show = lines.index("Show")
        assert lines[show - 1] == f'Type "{expected_command}"', tape_file
        assert lines[show + 1 : show + 3] == ["Sleep 300ms", "Enter"], tape_file

        if tape_file not in build["TUI_DEMOS"]:
            commands = extract_commands_from_tape(tape, REPO_ROOT)
            assert commands[0] == expected_command, tape_file
            assert "wt switch b" not in commands, tape_file


def test_docs_demos_hide_all_setup_and_cleanup_from_the_recording() -> None:
    """VHS pre-roll only works when Hide is the first executable directive."""
    build = runpy.run_path(str(DEMOS_DIR / "build"))
    inert_directives = ("Set ", "Output ", "Require ")

    for tape_file, _, _ in build["DOCS_DEMOS"]:
        rendered = render_tape(DEMOS_DIR / "tapes" / tape_file, {}, REPO_ROOT)
        assert rendered is not None
        lines = [
            line.strip()
            for line in rendered.splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]
        first_executable = next(
            line for line in lines if not line.startswith(inert_directives)
        )
        assert first_executable == "Hide", tape_file

        show = lines.index("Show")
        final_hide = len(lines) - 1 - lines[::-1].index("Hide")
        assert final_hide > show, tape_file


def test_mobile_core_demo_has_a_readable_dedicated_recording_contract() -> None:
    build = runpy.run_path(str(DEMOS_DIR / "build"))
    mobile_size = build["TARGETS"]["docs"]["sizes"]["wt-core-mobile"]
    assert mobile_size == DemoSize(width=576, height=432, fontsize=20)

    tape = DEMOS_DIR / "tapes" / "wt-core-mobile.tape"
    commands = extract_commands_from_tape(tape, REPO_ROOT)
    assert commands == [
        "wt list",
        "wt switch alpha",
        "wt switch --create api",
        "wt list",
        "wt remove",
    ]
    assert all(len(command) <= 24 for command in commands)
