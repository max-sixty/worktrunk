"""OCR-based validation for TUI demos.

TUI demos (Zellij, interactive UIs) can't be validated via text output because
VHS only captures the outer terminal, not content rendered inside terminal
multiplexers. Instead, we extract frames from the GIF and use OCR to verify
expected content appears.

Checkpoints specify a frame range rather than a single frame. The validator
scans frames within the range (sampling every N frames). Expected patterns must
share at least one sampled frame, while forbidden patterns must be absent from
every sampled frame. This makes validation resilient to timing shifts from UI
changes without allowing transient errors.

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
# frame within the range. Every forbidden pattern must be absent from every
# sampled frame.

TUI_CHECKPOINTS: dict[str, list[Checkpoint]] = {
    "wt-switch": [
        Checkpoint(
            start=360,
            end=455,
            expected=["Claude Code", "Opus", "acme.dashboard"],
            forbidden=[
                "Not logged in",
                "Unknown command",
                "Fable 5 is now",
                "Tackle your toughest",
            ],
        ),
    ],
    "wt-statusline": [
        Checkpoint(
            start=280,
            end=410,
            expected=["Claude Code", "Opus", "acme.alpha"],
            forbidden=[
                "Not logged in",
                "Unknown command",
                "Fable 5 is now",
                "Tackle your toughest",
            ],
        ),
    ],
    "wt-zellij-omnibus": [
        # Claude UI visible on TAB 1 (api) — shows model name and task.
        # Range covers the window where Claude's UI is rendered and stable.
        # Patterns kept minimal (just "Opus" + "acme") since Claude's UI
        # layout shifts across versions — task text may wrap or truncate.
        Checkpoint(
            start=150,
            end=450,
            expected=["Opus", "acme"],
            forbidden=[
                "command not found",
                "Unknown command",
                "Not logged in",
                "Fable 5 is now",
                "Tackle your toughest",
            ],
        ),
        # Claude UI visible on TAB 2 (billing), without referral or model ads.
        Checkpoint(
            start=550,
            end=700,
            expected=["Opus", "billing"],
            forbidden=[
                "Not logged in",
                "Share Claude Code",
                "Fable 5 is now",
                "Tackle your toughest",
            ],
        ),
        # The API agent adds a test. Commit generation should describe that
        # diff rather than replaying the feature-tab fixture's message.
        Checkpoint(
            start=1400,
            end=1750,
            expected=["expand", "coverage"],
            forbidden=["user settings module", "script -q"],
        ),
        # The feature push uses a local demo remote internally, but its
        # disposable filesystem path must never appear in the recording.
        Checkpoint(
            start=1000,
            end=1350,
            expected=["Removing feature"],
            forbidden=["/var/folders/", "wt-demo-"],
        ),
        # Near end — wt list --full showing all worktrees.
        # "billing" omitted: depends on timing of when the branch appears
        # in the list relative to the frame window.
        Checkpoint(
            start=1650,
            end=1850,
            expected=["Branch", "main"],
            forbidden=[
                "CONFLICT",
                "error:",
                "failed",
                "cargo test 2>&1",
                "cargo test -- --list",
                "script -q",
            ],
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


def extract_frames(
    gif_path: Path, frames: list[int], out_dir: Path
) -> dict[int, Path]:
    """Extract multiple frames from a GIF in a single ffmpeg pass.

    Returns a mapping from frame number to extracted PNG path.
    """
    if not frames:
        return {}

    # Build select filter: select='eq(n,150)+eq(n,160)+eq(n,170)+...'
    select_expr = "+".join(f"eq(n\\,{f})" for f in frames)
    pattern = str(out_dir / "frame_%04d.png")

    result = subprocess.run(
        [
            "ffmpeg",
            "-loglevel", "error",
            "-i", str(gif_path),
            "-vf", f"select='{select_expr}'",
            "-fps_mode", "vfr",
            str(pattern),
        ],
        capture_output=True,
    )
    if result.returncode != 0:
        return {}

    # ffmpeg numbers output files sequentially (frame_0001.png, frame_0002.png, ...)
    return {
        frame: out_dir / f"frame_{i + 1:04d}.png"
        for i, frame in enumerate(frames)
        if (out_dir / f"frame_{i + 1:04d}.png").exists()
    }


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


def _check_patterns(
    text: str,
    expected: list[str],
    forbidden: list[str],
) -> tuple[bool, list[str]]:
    """Check text against expected/forbidden patterns.

    Returns (passed, errors).
    """
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

    Extracts all sampled frames in one ffmpeg call, then OCRs each. Expected
    patterns must share a frame; forbidden patterns must be absent throughout
    the range.

    Returns (passed, detail_message).
    """
    frame_numbers = list(range(checkpoint.start, checkpoint.end + 1, checkpoint.step))
    frame_paths = extract_frames(gif_path, frame_numbers, work_dir)

    if not frame_paths:
        label = f"frames {checkpoint.start}-{checkpoint.end}"
        return False, f"failed to extract {label}"

    best_errors: list[str] = []
    frames_checked = 0
    matched_frame: int | None = None

    for frame in frame_numbers:
        frame_path = frame_paths.get(frame)
        if frame_path is None:
            continue

        frames_checked += 1
        text = ocr_image(frame_path)
        if not text:
            continue

        text_lower = text.lower()
        for pattern in checkpoint.forbidden:
            if pattern.lower() in text_lower:
                return False, f"forbidden '{pattern}' present at frame {frame}"

        passed, errors = _check_patterns(text, checkpoint.expected, [])
        if passed and matched_frame is None:
            matched_frame = frame
        if not best_errors or len(errors) < len(best_errors):
            best_errors = errors

    label = f"frames {checkpoint.start}-{checkpoint.end}"
    if not frames_checked:
        return False, f"no readable frames in {label}"
    if matched_frame is not None:
        return True, f"matched at frame {matched_frame} ({frames_checked} checked)"
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
