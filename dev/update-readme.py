#!/usr/bin/env python3
"""Update README.md with content from snapshot files.

This script manages README sections that reference snapshot test output.

USAGE:

1. Update README from snapshots:
   python dev/update-readme.py

   Replaces marked sections with normalized snapshot content:
   - Strips ANSI color codes
   - Converts [SHA] to a1b2c3d
   - Normalizes temp paths to relative paths

2. Check for staleness (useful in CI):
   python dev/update-readme.py --check

MARKER FORMAT (add manually to README):

<!-- README:snapshot:path/to/snapshot.snap -->
```bash
$ command
... content from snapshot ...
```
<!-- README:end -->
"""

import re
import subprocess
import sys
from pathlib import Path


def strip_ansi(text: str) -> str:
    """Remove ANSI escape codes from text.

    Handles both actual escape sequences (\x1b[...) and literal bracket
    notation ([0m, [1m, etc.) as stored in snapshot files.
    """
    # Actual ANSI escape sequences
    text = re.sub(r'\x1b\[[0-9;]*m', '', text)
    # Literal bracket notation (as stored in snapshots)
    text = re.sub(r'\[[0-9;]*m', '', text)
    return text


def parse_snapshot(path: Path) -> str:
    """Extract content from an insta snapshot file."""
    content = path.read_text()

    # Remove YAML front matter
    if content.startswith('---'):
        # Find the end of YAML front matter
        parts = content.split('---', 2)
        if len(parts) >= 3:
            content = parts[2].strip()

    # Handle insta_cmd format with stdout/stderr sections
    if '----- stdout -----' in content:
        stdout_match = re.search(r'----- stdout -----\n(.*?)----- stderr -----', content, re.DOTALL)
        stderr_match = re.search(r'----- stderr -----\n(.*?)(?:\Z|----- )', content, re.DOTALL)

        # Use stdout if it has content (worktrunk output), otherwise use stderr
        # Don't combine them - they're not temporally ordered
        stdout_content = stdout_match.group(1).rstrip() if stdout_match else ""
        stderr_content = stderr_match.group(1).rstrip() if stderr_match else ""

        if stdout_content:
            content = stdout_content
        else:
            content = stderr_content

    # Strip ANSI codes
    content = strip_ansi(content)

    return content


def get_help_output(command: str) -> str:
    """Run a command and capture its help output.

    For long help (--help), extracts only the Usage section (before
    documentation sections like "Operation", "Hooks", etc).
    """
    args = command.split()
    result = subprocess.run(
        args,
        capture_output=True,
        text=True,
        cwd=Path(__file__).parent.parent
    )
    # Help goes to stdout
    output = result.stdout if result.stdout else result.stderr
    output = strip_ansi(output).strip()

    # For --help, extract only the Usage section
    # Stop at first documentation section (indicated by title without indentation)
    if '--help' in command:
        lines = output.split('\n')
        result_lines = []
        in_header = True

        for line in lines:
            # Check if we've reached a documentation section
            # These are lines that start without indentation and aren't options
            if not in_header and line and not line.startswith(' ') and not line.startswith('-'):
                # Check for section headers (e.g., "Operation", "Hooks", "Examples")
                if line[0].isupper() and not line.startswith('Usage:') and ':' not in line:
                    break

            result_lines.append(line)

            # We're past the title line after first line
            if 'Usage:' in line:
                in_header = False

        return '\n'.join(result_lines).strip()

    return output


def normalize_for_readme(content: str) -> str:
    """Normalize snapshot output for README display.

    Converts test placeholders to realistic-looking values:
    - [SHA] -> a1b2c3d (realistic commit hash)
    - [TMPDIR]/test-repo.branch -> ../repo.branch/
    - ./test-repo -> ./repo
    """
    # Replace SHA/HASH placeholders with a consistent realistic hash
    content = re.sub(r'\[SHA\]', 'a1b2c3d', content)
    content = re.sub(r'\[HASH\]', 'a1b2c3d', content)

    # Replace temp dir paths with readable relative paths
    # Match [TMPDIR]/test-repo.branch (no trailing slash in replacement)
    content = re.sub(
        r'\[TMPDIR\]/test-repo\.(\S+?)(?=/|\s|$)',
        r'../repo.\1',
        content
    )
    # Also handle other temp path patterns
    content = re.sub(
        r'/(?:var/folders|tmp|private/tmp)[^/\s]*/[^/\s]*/test-repo\.(\S+?)(?=/|\s|$)',
        r'../repo.\1',
        content
    )

    # Replace [REPO] placeholder with repo
    content = re.sub(r'\[REPO\]', '../repo', content)

    return content


def update_readme(readme_path: Path, dry_run: bool = False) -> list[tuple[str, str, str]]:
    """Update README with content from markers.

    Returns list of (marker_type, identifier, status) tuples.
    """
    content = readme_path.read_text()
    updates = []

    # Pattern for snapshot markers
    # <!-- README:snapshot:path/to/file.snap -->
    # ```bash
    # $ command
    # content
    # ```
    # <!-- README:end -->
    snapshot_pattern = re.compile(
        r'(<!-- README:snapshot:([^\s]+) -->)\n'
        r'```(\w+)\n'
        r'(\$ [^\n]+\n)?'  # Optional command line
        r'(.*?)'
        r'```\n'
        r'(<!-- README:end -->)',
        re.DOTALL
    )

    # Pattern for help markers - finds first code block in section
    help_pattern = re.compile(
        r'(<!-- README:help:([^\n]+) -->)\n'
        r'(.*?)'  # Content before first code block
        r'(```\n)'
        r'(.*?)'  # Code block content
        r'(```)\n'
        r'(.*?)'  # Content after code block until end marker
        r'(<!-- README:end -->)',
        re.DOTALL
    )

    project_root = readme_path.parent

    def replace_snapshot(match):
        start_marker = match.group(1)
        snap_path = match.group(2)
        lang = match.group(3)
        command = match.group(4) or ""
        end_marker = match.group(6)

        full_path = project_root / snap_path
        if not full_path.exists():
            updates.append(('snapshot', snap_path, f'NOT FOUND: {full_path}'))
            return match.group(0)

        try:
            new_content = parse_snapshot(full_path)
            new_content = normalize_for_readme(new_content)
            updates.append(('snapshot', snap_path, 'updated'))
            return f'{start_marker}\n```{lang}\n{command}{new_content}\n```\n{end_marker}'
        except Exception as e:
            updates.append(('snapshot', snap_path, f'ERROR: {e}'))
            return match.group(0)

    def replace_help(match):
        start_marker = match.group(1)
        command = match.group(2)
        before_code = match.group(3)
        code_start = match.group(4)
        after_code = match.group(7)
        end_marker = match.group(8)

        try:
            new_content = get_help_output(command)
            updates.append(('help', command, 'updated'))
            return f'{start_marker}\n{before_code}{code_start}{new_content}\n```\n{after_code}{end_marker}'
        except Exception as e:
            updates.append(('help', command, f'ERROR: {e}'))
            return match.group(0)

    # Apply replacements
    new_content = snapshot_pattern.sub(replace_snapshot, content)
    new_content = help_pattern.sub(replace_help, new_content)

    if not dry_run and new_content != content:
        readme_path.write_text(new_content)

    return updates


def check_staleness(readme_path: Path) -> list[tuple[str, str, bool]]:
    """Check if README sections are stale compared to their source snapshots.

    Returns list of (snap_path, status, is_different) tuples.
    """
    content = readme_path.read_text()
    results = []

    # Pattern for snapshot markers
    snapshot_pattern = re.compile(
        r'<!-- README:snapshot:([^\s]+) -->\n'
        r'```\w*\n'
        r'(?:\$ [^\n]+\n)?'
        r'(.*?)'
        r'```\n'
        r'<!-- README:end -->',
        re.DOTALL
    )

    project_root = readme_path.parent

    for match in snapshot_pattern.finditer(content):
        snap_path = match.group(1)
        current_content = match.group(2).strip()

        full_path = project_root / snap_path
        if not full_path.exists():
            results.append((snap_path, 'NOT FOUND', True))
            continue

        try:
            snap_content = parse_snapshot(full_path)
            snap_content = normalize_for_readme(snap_content)
            is_different = snap_content.strip() != current_content
            status = 'needs review' if is_different else 'ok'
            results.append((snap_path, status, is_different))
        except Exception as e:
            results.append((snap_path, f'ERROR: {e}', True))

    return results


def main():
    import argparse

    parser = argparse.ArgumentParser(description='Update README.md from snapshot files')
    parser.add_argument('--check', action='store_true',
                        help='Check if sections are stale without updating')
    parser.add_argument('--dry-run', action='store_true',
                        help='Show what would be updated without making changes')
    parser.add_argument('--readme', type=Path, default=Path('README.md'),
                        help='Path to README.md')

    args = parser.parse_args()

    readme_path = args.readme
    if not readme_path.is_absolute():
        readme_path = Path(__file__).parent.parent / readme_path

    if not readme_path.exists():
        print(f"Error: {readme_path} not found", file=sys.stderr)
        sys.exit(1)

    if args.check:
        results = check_staleness(readme_path)

        if not results:
            print("No markers found in README.md")
            sys.exit(0)

        has_stale = False
        for snap_path, status, is_different in results:
            indicator = "⚠️ " if is_different else "✅"
            print(f"{indicator} {snap_path}: {status}")
            if is_different:
                has_stale = True

        if has_stale:
            print("\nSome sections may need manual review")
            sys.exit(1)
        else:
            print("\nAll sections are up to date")
    else:
        updates = update_readme(readme_path, dry_run=args.dry_run)

        if not updates:
            print("No markers found in README.md")
            sys.exit(0)

        for marker_type, identifier, status in updates:
            print(f"[{marker_type}] {identifier}: {status}")

        if args.dry_run:
            print("\nDry run - no changes made")


if __name__ == '__main__':
    main()
