"""OCR-based validation for TUI demos.

TUI demos (Zellij, interactive UIs) can't be validated via text output because
VHS only captures the outer terminal, not content rendered inside terminal
multiplexers. Instead, we extract frames from the GIF and use OCR to verify
expected content appears.

Checkpoints specify a frame range rather than a single frame. The validator
scans frames within the range (sampling every N frames) and passes the
checkpoint if ANY frame in the range matches all expected patterns while
containing none of the forbidden patterns. This makes validation resilient
to timing shifts from UI changes.

Usage:
    from shared.validation import validate_tui_demo, TUI_CHECKPOINTS

    # Validate after building
    errors = validate_tui_demo("wt-zellij-omnibus", gif_path)
    if errors:
        print("Validation failed:", errors)
"""

from __future__ import annotations

import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class Checkpoint:
    """A validation checkpoint that scans a range of frames."""

    start: int
    end: int
    expected: list[str] = field(default_factory=list)
    forbidden: list[str] = field(default_factory=list)
    step: int = 10


# Checkpoint definitions per TUI demo.
# Ranges are calibrated from actual GIF content at 30fps.
# Expected patterns must ALL be present (case-insensitive) in at least one
# frame within the range. Forbidden patterns must ALL be absent.

TUI_CHECKPOINTS: dict[str, list[Checkpoint]] = {
    "wt-zellij-omnibus": [
        # Claude UI visible on TAB 1 (api) — shows model name and task.
        # Range covers the window where Claude's UI is rendered and stable.
        # Patterns kept minimal (just "Opus" + "acme") since Claude's UI
        # layout shifts across versions — task text may wrap or truncate.
        Checkpoint(
            start=150,
            end=350,
            expected=["Opus", "acme"],
            forbidden=["command not found", "Unknown command"],
        ),
        # Near end — wt list --full showing all worktrees.
        # "billing" omitted: depends on timing of when the branch appears
        # in the list relative to the frame window.
        Checkpoint(
            start=1650,
            end=1850,
            expected=["Branch", "main"],
            forbidden=["CONFLICT", "error:", "failed"],
        ),
    ],
}


def check_dependencies() -> list[str]:
    """Check that required tools are available. Returns list of missing tools."""
    missing = []
    for cmd in ["ffmpeg", "tesseract"]:
        result = subprocess.run(
            ["which", cmd], capture_output=True, text=True
        )
        if result.returncode != 0:
            missing.append(cmd)
    return missing


def extract_frame(gif_path: Path, frame_number: int, output_path: Path) -> bool:
    """Extract a single frame from a GIF. Returns True on success."""
    result = subprocess.run(
        [
            "ffmpeg",
            "-loglevel", "error",
            "-i", str(gif_path),
            "-vf", f"select=eq(n\\,{frame_number})",
            "-vframes", "1",
            "-update", "1",
            str(output_path),
        ],
        capture_output=True,
    )
    return result.returncode == 0 and output_path.exists()


def ocr_image(image_path: Path) -> str:
    """Run OCR on an image and return the extracted text."""
    with tempfile.NamedTemporaryFile(suffix=".txt", delete=False) as f:
        output_base = f.name[:-4]  # Remove .txt suffix for tesseract

    result = subprocess.run(
        ["tesseract", str(image_path), output_base, "-l", "eng"],
        capture_output=True,
    )

    output_path = Path(f"{output_base}.txt")
    if result.returncode == 0 and output_path.exists():
        text = output_path.read_text()
        output_path.unlink()
        return text
    return ""


def _check_frame(
    gif_path: Path,
    frame_number: int,
    expected: list[str],
    forbidden: list[str],
    work_dir: Path,
) -> tuple[bool, list[str]]:
    """Check a single frame against expected/forbidden patterns.

    Returns (passed, errors). A frame passes when all expected patterns are
    present AND no forbidden patterns are found.
    """
    frame_path = work_dir / f"frame_{frame_number}.png"
    if not extract_frame(gif_path, frame_number, frame_path):
        return False, [f"Failed to extract frame {frame_number}"]

    text = ocr_image(frame_path)
    if not text:
        return False, [f"OCR returned no text for frame {frame_number}"]

    text_lower = text.lower()
    errors = []

    for pattern in expected:
        if pattern.lower() not in text_lower:
            errors.append(f"'{pattern}' not found")

    for pattern in forbidden:
        if pattern.lower() in text_lower:
            errors.append(f"forbidden '{pattern}' present")

    return len(errors) == 0, errors


def validate_checkpoint(
    gif_path: Path,
    checkpoint: Checkpoint,
    work_dir: Path,
) -> tuple[bool, str]:
    """Validate a checkpoint by scanning its frame range.

    Returns (passed, detail_message).
    """
    best_errors: list[str] = []
    frames_checked = 0

    for frame in range(checkpoint.start, checkpoint.end + 1, checkpoint.step):
        frames_checked += 1
        passed, errors = _check_frame(
            gif_path, frame, checkpoint.expected, checkpoint.forbidden, work_dir
        )
        if passed:
            return True, f"matched at frame {frame} ({frames_checked} checked)"
        # Track the attempt with fewest errors (closest to passing)
        if not best_errors or len(errors) < len(best_errors):
            best_errors = errors

    label = f"frames {checkpoint.start}-{checkpoint.end}"
    return False, f"no match in {label} ({frames_checked} checked): {'; '.join(best_errors)}"


def validate_tui_demo(demo_name: str, gif_path: Path) -> list[str]:
    """Validate a TUI demo GIF against its checkpoints.

    Returns list of error messages. Empty list means validation passed.
    """
    if demo_name not in TUI_CHECKPOINTS:
        return [f"No checkpoints defined for demo: {demo_name}"]

    if not gif_path.exists():
        return [f"GIF not found: {gif_path}"]

    missing = check_dependencies()
    if missing:
        return [f"Missing required tools: {', '.join(missing)}"]

    checkpoints = TUI_CHECKPOINTS[demo_name]
    all_errors = []

    with tempfile.TemporaryDirectory(prefix="wt-validate-") as work_dir:
        work_path = Path(work_dir)

        for checkpoint in checkpoints:
            passed, detail = validate_checkpoint(gif_path, checkpoint, work_path)
            if not passed:
                all_errors.append(detail)

    return all_errors


def validate_tui_demo_verbose(demo_name: str, gif_path: Path) -> tuple[bool, str]:
    """Validate a TUI demo with verbose output.

    Returns (success, output_message).
    """
    lines = [f"Validating {demo_name}: {gif_path}"]

    if demo_name not in TUI_CHECKPOINTS:
        return False, f"No checkpoints defined for demo: {demo_name}"

    if not gif_path.exists():
        return False, f"GIF not found: {gif_path}"

    missing = check_dependencies()
    if missing:
        return False, f"Missing required tools: {', '.join(missing)}"

    checkpoints = TUI_CHECKPOINTS[demo_name]
    all_passed = True

    with tempfile.TemporaryDirectory(prefix="wt-validate-") as work_dir:
        work_path = Path(work_dir)

        for checkpoint in checkpoints:
            passed, detail = validate_checkpoint(gif_path, checkpoint, work_path)
            label = f"frames {checkpoint.start}-{checkpoint.end}"
            if passed:
                lines.append(f"  ✓ {label}: {detail}")
            else:
                lines.append(f"  ✗ {label}: {detail}")
                all_passed = False

    if all_passed:
        lines.append("✓ All checkpoints passed")
    else:
        lines.append("✗ Some checkpoints failed")

    return all_passed, "\n".join(lines)
