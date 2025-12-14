"""Shared infrastructure for demo recording scripts."""

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from themes import THEMES, format_theme_for_vhs

REAL_HOME = Path.home()


@dataclass
class DemoEnv:
    """Isolated demo environment with its own repo and home directory."""

    name: str
    out_dir: Path
    repo_name: str = "worktrunk"

    @property
    def root(self) -> Path:
        return self.out_dir / f".demo-{self.name}"

    @property
    def home(self) -> Path:
        return self.root

    @property
    def work_base(self) -> Path:
        return self.home / "w"

    @property
    def repo(self) -> Path:
        return self.work_base / self.repo_name

    @property
    def bare_remote(self) -> Path:
        return self.root / "remote.git"


def run(cmd, cwd=None, env=None, check=True, capture=False):
    """Run a command."""
    result = subprocess.run(
        cmd, cwd=cwd, env=env, check=check,
        capture_output=capture, text=True
    )
    return result.stdout if capture else None


def git(args, cwd=None, env=None):
    """Run git command."""
    run(["git"] + args, cwd=cwd, env=env)


def render_tape(template_path: Path, output_path: Path, replacements: dict) -> bool:
    """Render a VHS tape template with variable substitutions.

    Args:
        template_path: Path to the .tape template file
        output_path: Path to write the rendered .tape file
        replacements: Dict of {{VAR}} -> value replacements

    Returns:
        True if successful, False if template doesn't exist
    """
    if not template_path.exists():
        print(f"Warning: {template_path} not found, skipping VHS recording")
        return False

    template = template_path.read_text()
    rendered = template
    for key, value in replacements.items():
        rendered = rendered.replace(f"{{{{{key}}}}}", str(value))
    output_path.write_text(rendered)
    return True


def record_vhs(tape_path: Path, vhs_binary: str = "vhs"):
    """Record a demo GIF using VHS."""
    run([vhs_binary, str(tape_path)], check=True)


def build_wt(repo_root: Path):
    """Build the wt binary."""
    print("Building wt binary...")
    run(["cargo", "build", "--quiet"], cwd=repo_root)
