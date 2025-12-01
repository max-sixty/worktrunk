#!/usr/bin/env python3
"""Build script for worktrunk demo GIF and text output."""

import os
import re
import shutil
import subprocess
from datetime import datetime, timedelta
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
REPO_ROOT = SCRIPT_DIR.parent
DEMO_DIR = SCRIPT_DIR / "wt-demo"
FIXTURES_DIR = DEMO_DIR / "fixtures"
OUT_DIR = DEMO_DIR / "out"
TAPE_TEMPLATE = DEMO_DIR / "demo.tape"
TAPE_RENDERED = OUT_DIR / ".rendered.tape"
STARSHIP_CONFIG = OUT_DIR / "starship.toml"
OUTPUT_GIF = OUT_DIR / "wt-demo.gif"
LOG = OUT_DIR / "record.log"

REAL_HOME = Path.home()


class DemoEnv:
    """Isolated demo environment with its own repo and home directory."""

    def __init__(self, name: str):
        self.name = name
        self.root = OUT_DIR / f".demo-{name}"
        self.home = self.root
        self.work_base = self.home / "w"
        self.repo = self.work_base / "acme"
        self.bare_remote = self.root / "remote.git"


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


def commit_dated(repo, message, offset):
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
    git(["-C", str(repo), "commit", "-qm", message], env=env)


def prepare_repo(env: DemoEnv):
    """Set up the demo repository with branches and worktrees."""
    # Clean previous
    shutil.rmtree(env.root, ignore_errors=True)
    legacy = REPO_ROOT / ".demo"
    if legacy.exists():
        shutil.rmtree(legacy)

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
    (env.repo / "README.md").write_text("# Worktrunk demo\n\nThis repo is generated automatically.\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "Initial demo commit", "7d")
    git(["-C", str(env.repo), "branch", "-m", "main"])
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

    # Project hooks
    (env.repo / ".config").mkdir()
    (env.repo / ".config" / "wt.toml").write_text(
        '[pre-merge]\ntest = "cargo nextest run"\n'
    )
    git(["-C", str(env.repo), "add", ".config/wt.toml"])
    commit_dated(env.repo, "Add project hooks", "5d")
    git(["-C", str(env.repo), "push", "-q"])

    # Mock gh CLI
    bin_dir = env.home / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    gh_mock = bin_dir / "gh"
    shutil.copy(FIXTURES_DIR / "gh-mock.sh", gh_mock)
    gh_mock.chmod(0o755)

    # Build wt binary
    run(["cargo", "build", "--quiet"], cwd=REPO_ROOT)

    # User config
    config_dir = env.home / ".config" / "worktrunk"
    config_dir.mkdir(parents=True)
    project_id = str(env.bare_remote).removesuffix(".git")
    (config_dir / "config.toml").write_text(f'''[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]

[projects."{project_id}"]
approved-commands = ["cargo nextest run"]
''')

    # Extra branches (no worktrees)
    git(["-C", str(env.repo), "branch", "docs/readme"])
    git(["-C", str(env.repo), "branch", "spike/search"])

    # Create feature branches
    create_branch_alpha(env)
    create_branch_beta(env)

    # Commit to main after beta so beta is behind
    readme = env.repo / "README.md"
    readme.write_text(readme.read_text() + "# Development\nSee CONTRIBUTING.md for guidelines.\n")
    (env.repo / "notes.md").write_text("# Notes\n")
    git(["-C", str(env.repo), "add", "README.md", "notes.md"])
    commit_dated(env.repo, "docs: add development section", "1d")
    git(["-C", str(env.repo), "push", "-q"])

    create_branch_hooks(env)


def create_branch_alpha(env: DemoEnv):
    """Create alpha branch with large diff and unpushed commits."""
    branch = "alpha"
    path = env.work_base / f"acme.{branch}"

    git(["-C", str(env.repo), "checkout", "-q", "-b", branch, "main"])

    # Initial README changes
    (env.repo / "README.md").write_text('''# Worktrunk demo

A demo repository for showcasing worktrunk features.

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
    readme.write_text(readme.read_text() + "\n# Contributing\nPRs welcome!\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "docs: add contributing section", "3d")

    readme.write_text(readme.read_text() + "\n# License\nMIT\n")
    git(["-C", str(env.repo), "add", "README.md"])
    commit_dated(env.repo, "docs: add license", "3d")

    git(["-C", str(env.repo), "push", "-u", "origin", branch, "-q"])
    git(["-C", str(env.repo), "checkout", "-q", "main"])
    git(["-C", str(env.repo), "worktree", "add", "-q", str(path), branch])

    # Unpushed commit
    readme = path / "README.md"
    readme.write_text(readme.read_text() + "# FAQ\n")
    git(["-C", str(path), "add", "README.md"])
    commit_dated(path, "docs: add FAQ section", "3d")

    # Working tree changes - large diff
    shutil.copy(FIXTURES_DIR / "alpha-readme.md", path / "README.md")
    (path / "scratch.rs").write_text("// scratch\n")


def create_branch_beta(env: DemoEnv):
    """Create beta branch with staged changes."""
    branch = "beta"
    path = env.work_base / f"acme.{branch}"

    git(["-C", str(env.repo), "checkout", "-q", "-b", branch, "main"])
    git(["-C", str(env.repo), "push", "-u", "origin", branch, "-q"])
    git(["-C", str(env.repo), "checkout", "-q", "main"])
    git(["-C", str(env.repo), "worktree", "add", "-q", str(path), branch])

    # Staged new file
    (path / "notes.txt").write_text("# TODO\n- Add caching\n")
    git(["-C", str(path), "add", "notes.txt"])


def create_branch_hooks(env: DemoEnv):
    """Create hooks branch with refactored lib.rs."""
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


def render_tape(env: DemoEnv):
    """Render the VHS tape template with variables."""
    template = TAPE_TEMPLATE.read_text()
    rendered = template.replace("{{DEMO_REPO}}", str(env.repo))
    rendered = rendered.replace("{{DEMO_HOME}}", str(env.home))
    rendered = rendered.replace("{{REAL_HOME}}", str(REAL_HOME))
    rendered = rendered.replace("{{STARSHIP_CONFIG}}", str(STARSHIP_CONFIG))
    rendered = rendered.replace("{{OUTPUT_GIF}}", str(OUTPUT_GIF))
    rendered = rendered.replace("{{TARGET_DEBUG}}", str(REPO_ROOT / "target" / "debug"))
    TAPE_RENDERED.write_text(rendered)


def record_text(demo_env: DemoEnv):
    """Record text output by running demo commands."""
    # Extract commands from tape
    commands = []
    for line in TAPE_TEMPLATE.read_text().splitlines():
        if line.startswith("Type "):
            cmd = line[5:].strip().strip('"').strip("'")
            # Skip setup commands (fish uses 'set -gx' instead of 'export')
            if not any(cmd.startswith(p) for p in ["set -gx", "export ", "eval ", "cd ", "clear", "exit", "starship init", "wt config shell init"]):
                commands.append(cmd)

    # Set up environment
    env = os.environ.copy()
    env.update({
        "LANG": "en_US.UTF-8",
        "LC_ALL": "en_US.UTF-8",
        "COLUMNS": "160",
        "RUSTUP_HOME": str(REAL_HOME / ".rustup"),
        "CARGO_HOME": str(REAL_HOME / ".cargo"),
        "HOME": str(demo_env.home),
        "PATH": f"{REPO_ROOT}/target/debug:{demo_env.home}/bin:{os.environ['PATH']}",
        "STARSHIP_CONFIG": str(STARSHIP_CONFIG),
        "STARSHIP_CACHE": str(demo_env.root / "starship-cache"),
        "WT_PROGRESSIVE": "false",
        "NO_COLOR": "1",
        "CLICOLOR": "0",
        "NEXTEST_STATUS_LEVEL": "none",
        "NEXTEST_FINAL_STATUS_LEVEL": "flaky",
        "NEXTEST_NO_FAIL_FAST": "1",
    })

    (demo_env.root / "starship-cache").mkdir(exist_ok=True)

    # Run commands and capture output using fish with shell init
    script = '''wt config shell init fish | source
'''
    for cmd in commands:
        script += f'{cmd}\n'
        if cmd.startswith("wt merge"):
            script += 'sleep 2\n'

    result = subprocess.run(
        ["fish", "-c", script],
        cwd=demo_env.repo, env=env,
        capture_output=True, text=True
    )
    output = [result.stdout + result.stderr]

    # Clean output
    raw = "".join(output)
    clean = re.sub(r"\x1B\[[0-9;?]*[A-Za-z]", "", raw)  # Strip ANSI
    clean = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", "", clean)  # Strip control chars
    clean = clean.replace("^D", "")

    (OUT_DIR / "run.txt").write_text(clean)


def record_vhs():
    """Record the demo GIF using VHS."""
    with open(LOG, "w") as f:
        run(["vhs", str(TAPE_RENDERED)], check=True)


def main():
    # Check dependencies
    for cmd in ["wt", "vhs", "starship"]:
        if not shutil.which(cmd):
            raise SystemExit(f"Missing dependency: {cmd}")

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    # Copy starship config
    shutil.copy(FIXTURES_DIR / "starship.toml", STARSHIP_CONFIG)

    # Create separate environments for text and VHS recording
    # so they don't interfere with each other (different LLM-generated commits)
    text_env = DemoEnv("text")
    vhs_env = DemoEnv("vhs")

    prepare_repo(text_env)
    record_text(text_env)

    prepare_repo(vhs_env)
    render_tape(vhs_env)
    record_vhs()

    # Cleanup
    TAPE_RENDERED.unlink(missing_ok=True)

    print(f"GIF saved to {OUTPUT_GIF}")
    print(f"Text log saved to {OUT_DIR / 'run.txt'}")
    print(f"Log: {LOG}")


if __name__ == "__main__":
    main()
