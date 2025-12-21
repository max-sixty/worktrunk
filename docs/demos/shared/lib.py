"""Shared infrastructure for demo recording scripts."""

import os
import shutil
import subprocess
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

REAL_HOME = Path.home()
FIXTURES_DIR = Path(__file__).parent / "fixtures"


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


def commit_dated(repo: Path, message: str, offset: str, env_extra: dict = None):
    """Commit with a date offset like '7d' or '2H'."""
    now = datetime.now()
    if offset.endswith("d"):
        delta = timedelta(days=int(offset[:-1]))
    elif offset.endswith("H"):
        delta = timedelta(hours=int(offset[:-1]))
    else:
        raise ValueError(f"Unknown offset format: {offset}")

    date_str = (now - delta).strftime("%Y-%m-%dT%H:%M:%S")
    env = os.environ.copy()
    env["GIT_AUTHOR_DATE"] = date_str
    env["GIT_COMMITTER_DATE"] = date_str
    env["SKIP_DEMO_HOOK"] = "1"
    if env_extra:
        env.update(env_extra)
    git(["-C", str(repo), "commit", "-qm", message], env=env)


def prepare_base_repo(env: DemoEnv, repo_root: Path):
    """Set up the base demo repository with Rust project.

    Creates:
    - Git repo with initial commit
    - Rust project (Cargo.toml, lib.rs, Cargo.lock)
    - Mock gh CLI for CI status
    - bat wrapper for syntax highlighting
    - User config directory

    Demos should call this first, then add their own:
    - Project hooks config (.config/wt.toml)
    - Branches and worktrees
    - Additional mock CLIs
    - Approved commands in user config
    """
    # Clean previous
    shutil.rmtree(env.root, ignore_errors=True)

    env.root.mkdir(parents=True)
    env.work_base.mkdir(parents=True)
    env.repo.mkdir(parents=True)

    # Init bare remote
    run(["git", "init", "--bare", "-q", str(env.bare_remote)])

    # Init main repo
    git(["-C", str(env.repo), "init", "-q"])
    git(["-C", str(env.repo), "config", "user.name", "Worktrunk Demo"])
    git(["-C", str(env.repo), "config", "user.email", "demo@example.com"])
    git(["-C", str(env.repo), "config", "commit.gpgsign", "false"])

    # Initial commit
    (env.repo / "README.md").write_text("# Acme App\n\nA demo application.\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "Initial commit", "7d")
    git(["-C", str(env.repo), "branch", "-m", "main"])
    # Use local bare repo as remote (GitHub URLs cause VHS to hang waiting for SSH)
    git(["-C", str(env.repo), "remote", "add", "origin", str(env.bare_remote)])
    git(["-C", str(env.repo), "push", "-u", "origin", "main", "-q"])

    # Rust project
    (env.repo / "Cargo.toml").write_text(
        "[package]\nname = \"acme\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"
    )
    (env.repo / "src").mkdir()
    shutil.copy(FIXTURES_DIR / "lib.rs", env.repo / "src" / "lib.rs")
    (env.repo / ".gitignore").write_text("/target\n")
    git(["-C", str(env.repo), "add", ".gitignore", "Cargo.toml", "src/"])
    commit_dated(env.repo, "Add Rust project with tests", "6d")

    # Build to create Cargo.lock
    run(["cargo", "build", "--release", "-q"], cwd=env.repo, check=False)
    git(["-C", str(env.repo), "add", "Cargo.lock"])
    commit_dated(env.repo, "Add Cargo.lock", "6d")
    git(["-C", str(env.repo), "push", "-q"])

    # Mock CLI tools
    bin_dir = env.home / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)

    # bat wrapper for syntax highlighting (alias cat to bat for toml files)
    bat_wrapper = bin_dir / "cat"
    bat_wrapper.write_text("""#!/bin/bash
# Use bat for syntax highlighting if file is toml
if [[ "$1" == *.toml ]]; then
    exec bat --style=plain --paging=never "$@"
else
    exec /bin/cat "$@"
fi
""")
    bat_wrapper.chmod(0o755)

    # Build wt binary
    build_wt(repo_root)

    # User config directory (demos add their own config.toml)
    config_dir = env.home / ".config" / "worktrunk"
    config_dir.mkdir(parents=True)

    # Project config directory
    (env.repo / ".config").mkdir(exist_ok=True)


def setup_gh_mock(env: DemoEnv, fixtures_dir: Path):
    """Set up gh CLI mock from demo's fixtures directory."""
    bin_dir = env.home / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    gh_mock = bin_dir / "gh"
    shutil.copy(fixtures_dir / "gh-mock.sh", gh_mock)
    gh_mock.chmod(0o755)


def prepare_demo_repo(env: DemoEnv, repo_root: Path, hooks_config: str = None):
    """Set up a full demo repository with varied branches.

    Creates a rich repo for great `wt list` output:
    - Git repo with Rust project
    - Mock gh CLI for CI status (generic version)
    - bat wrapper for syntax highlighting
    - Extra branches without worktrees (docs/readme, spike/search)
    - alpha: large diff, unpushed commits, behind main
    - beta: staged changes, behind main
    - hooks: no remote, staged+unstaged changes

    Args:
        env: Demo environment
        repo_root: Path to worktrunk repo for building wt
        hooks_config: Optional project hooks (.config/wt.toml) content.
                      If None, uses default pre-merge hook.

    After calling this, main is at the latest commit and worktrees exist for
    alpha, beta, hooks. Demos can then add their own branches/config.
    """
    # Base setup: git repo, Rust project, bat wrapper, wt binary
    prepare_base_repo(env, repo_root)

    # Project hooks
    if hooks_config is None:
        hooks_config = '[pre-merge]\ntest = "cargo nextest run"\n'
    (env.repo / ".config" / "wt.toml").write_text(hooks_config)
    git(["-C", str(env.repo), "add", ".config/wt.toml"])
    commit_dated(env.repo, "Add project hooks", "5d")
    git(["-C", str(env.repo), "push", "-q"])

    # Mock gh CLI with varied CI status per branch
    bin_dir = env.home / "bin"
    gh_mock = bin_dir / "gh"
    shutil.copy(FIXTURES_DIR / "gh-mock.sh", gh_mock)
    gh_mock.chmod(0o755)

    # Extra branches without worktrees (for --branches view)
    git(["-C", str(env.repo), "branch", "docs/readme"])
    git(["-C", str(env.repo), "branch", "spike/search"])

    # Create beta first (from current main, so it will be behind after main commit)
    _create_branch_beta(env)

    # Commit to main so beta is behind
    readme = env.repo / "README.md"
    readme.write_text(readme.read_text() + "\n## Development\n\nSee CONTRIBUTING.md for guidelines.\n")
    (env.repo / "notes.md").write_text("# Notes\n")
    git(["-C", str(env.repo), "add", "README.md", "notes.md"])
    commit_dated(env.repo, "docs: add development section", "1d")
    git(["-C", str(env.repo), "push", "-q"])

    # Create alpha and hooks after the main commit (so they're only ahead, not diverged)
    _create_branch_alpha(env)
    _create_branch_hooks(env)


def _create_branch_alpha(env: DemoEnv):
    """Create alpha branch with large diff and unpushed commits."""
    branch = "alpha"
    path = env.work_base / f"acme.{branch}"

    git(["-C", str(env.repo), "checkout", "-q", "-b", branch, "main"])

    # Initial README changes
    (env.repo / "README.md").write_text('''# Acme App

A demo application for showcasing worktrunk features.

## Features

- Fast worktree switching
- Integrated merge workflow
- Pre-merge test hooks
- LLM commit messages

## Getting Started

Run `wt list` to see all worktrees.
''')
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "docs: expand README", "3d")

    # More commits
    readme = env.repo / "README.md"
    readme.write_text(readme.read_text() + "\n## Contributing\n\nPRs welcome!\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "docs: add contributing section", "3d")

    readme.write_text(readme.read_text() + "\n## License\n\nMIT\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "docs: add license", "3d")

    git(["-C", str(env.repo), "push", "-u", "origin", branch, "-q"])
    git(["-C", str(env.repo), "checkout", "-q", "main"])
    git(["-C", str(env.repo), "worktree", "add", "-q", str(path), branch])

    # Unpushed commit
    readme = path / "README.md"
    readme.write_text(readme.read_text() + "## FAQ\n\n")
    git(["-C", str(path), "add", "README.md"])
    commit_dated(path, "docs: add FAQ section", "3d")

    # Working tree changes - large diff using shared fixture
    shutil.copy(FIXTURES_DIR / "alpha-readme.md", path / "README.md")
    (path / "scratch.rs").write_text("// scratch\n")


def _create_branch_beta(env: DemoEnv):
    """Create beta branch with staged changes and remote tracking."""
    branch = "beta"
    path = env.work_base / f"acme.{branch}"

    git(["-C", str(env.repo), "checkout", "-q", "-b", branch, "main"])
    git(["-C", str(env.repo), "push", "-u", "origin", branch, "-q"])
    git(["-C", str(env.repo), "checkout", "-q", "main"])
    git(["-C", str(env.repo), "worktree", "add", "-q", str(path), branch])

    # Staged new file
    (path / "notes.txt").write_text("# TODO\n- Add caching\n")
    git(["-C", str(path), "add", "notes.txt"])


def _create_branch_hooks(env: DemoEnv):
    """Create hooks branch with refactored lib.rs, no remote."""
    branch = "hooks"
    path = env.work_base / f"acme.{branch}"

    git(["-C", str(env.repo), "checkout", "-q", "-b", branch, "main"])
    shutil.copy(FIXTURES_DIR / "lib-hooks.rs", env.repo / "src" / "lib.rs")
    git(["-C", str(env.repo), "add", "src/lib.rs"])
    commit_dated(env.repo, "feat: add math operations, consolidate tests", "2H")

    # No push - no upstream
    git(["-C", str(env.repo), "checkout", "-q", "main"])
    git(["-C", str(env.repo), "worktree", "add", "-q", str(path), branch])

    # Staged then modified
    lib_rs = path / "src" / "lib.rs"
    lib_rs.write_text(lib_rs.read_text() + "// Division coming soon\n")
    git(["-C", str(path), "add", "src/lib.rs"])
    lib_rs.write_text(lib_rs.read_text() + "// TODO: add division\n")
