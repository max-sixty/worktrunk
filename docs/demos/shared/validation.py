"""OCR-based validation for TUI demos.

TUI demos (Zellij, interactive UIs) can't be validated via text output because
VHS only captures the outer terminal, not content rendered inside terminal
multiplexers. Instead, we extract key frames from the GIF and use OCR to verify
expected content appears.

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
from pathlib import Path

# Checkpoint definitions per TUI demo
# Format: {demo_name: [(frame_number, expected_patterns, forbidden_patterns), ...]}
#
# Frame numbers are calibrated from actual GIF content at 30fps.
# Expected patterns must ALL be present (case-insensitive).
# Forbidden patterns must ALL be absent (case-insensitive).

TUI_CHECKPOINTS: dict[str, list[tuple[int, list[str], list[str]]]] = {
    "wt-zellij-omnibus": [
        # Frame 50: After wt list - should see branch table, no permission dialog
        (50, ["Branch", "Status", "main", "hooks"], ["Allow?", "permission"]),
        # Frame 300: Claude UI visible - shows Opus, task prompt, search activity
        (300, ["Opus", "acme", "test"], ["command not found", "Unknown command"]),
        # Frame 1750: After wt merge - shows merge success
        (1750, ["Merged", "main", "commit"], ["CONFLICT", "error:", "failed"]),
    ],
    # Other TUI demos can be added here:
    # "wt-select": [...],
    # "wt-switch": [...],
    # "wt-statusline": [...],
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


def validate_checkpoint(
    gif_path: Path,
    frame_number: int,
    expected: list[str],
    forbidden: list[str],
    work_dir: Path,
) -> list[str]:
    """Validate a single checkpoint. Returns list of error messages."""
    errors = []

    # Extract frame
    frame_path = work_dir / f"frame_{frame_number}.png"
    if not extract_frame(gif_path, frame_number, frame_path):
        return [f"Failed to extract frame {frame_number}"]

    # OCR the frame
    text = ocr_image(frame_path)
    if not text:
        return [f"OCR failed for frame {frame_number}"]

    text_lower = text.lower()

    # Check expected patterns
    for pattern in expected:
        if pattern.lower() not in text_lower:
            errors.append(f"Expected pattern not found: '{pattern}'")

    # Check forbidden patterns
    for pattern in forbidden:
        if pattern.lower() in text_lower:
            errors.append(f"Forbidden pattern found: '{pattern}'")

    return errors


def validate_tui_demo(demo_name: str, gif_path: Path) -> list[str]:
    """Validate a TUI demo GIF against its checkpoints.

    Args:
        demo_name: Name of the demo (e.g., "wt-zellij-omnibus")
        gif_path: Path to the GIF file to validate

    Returns:
        List of error messages. Empty list means validation passed.
    """
    if demo_name not in TUI_CHECKPOINTS:
        return [f"No checkpoints defined for demo: {demo_name}"]

    if not gif_path.exists():
        return [f"GIF not found: {gif_path}"]

    # Check dependencies
    missing = check_dependencies()
    if missing:
        return [f"Missing required tools: {', '.join(missing)}"]

    checkpoints = TUI_CHECKPOINTS[demo_name]
    all_errors = []

    with tempfile.TemporaryDirectory(prefix="wt-validate-") as work_dir:
        work_path = Path(work_dir)

        for frame_number, expected, forbidden in checkpoints:
            errors = validate_checkpoint(
                gif_path, frame_number, expected, forbidden, work_path
            )
            if errors:
                all_errors.append(f"Frame {frame_number}: {'; '.join(errors)}")

    return all_errors


def validate_tui_demo_verbose(demo_name: str, gif_path: Path) -> tuple[bool, str]:
    """Validate a TUI demo with verbose output.

    Returns:
        (success, output_message)
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

        for frame_number, expected, forbidden in checkpoints:
            errors = validate_checkpoint(
                gif_path, frame_number, expected, forbidden, work_path
            )
            if errors:
                lines.append(f"  ✗ Frame {frame_number}")
                for error in errors:
                    lines.append(f"    - {error}")
                all_passed = False
            else:
                lines.append(f"  ✓ Frame {frame_number}")

    if all_passed:
        lines.append("✓ All checkpoints passed")
    else:
        lines.append("✗ Some checkpoints failed")

    return all_passed, "\n".join(lines)
