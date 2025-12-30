"""Shared infrastructure for demo recording scripts."""

import json
import os
import re
import shutil
import subprocess
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

from .themes import THEMES, format_theme_for_vhs

REAL_HOME = Path.home()
FIXTURES_DIR = Path(__file__).parent / "fixtures"

# Shared content for demos
VALIDATION_RS = '''//! Input validation utilities.

/// Validates that a number is positive.
pub fn is_positive(n: i32) -> bool {
    n > 0
}

/// Validates that a string is not empty.
pub fn is_non_empty(s: &str) -> bool {
    !s.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_positive() {
        assert!(is_positive(1));
        assert!(!is_positive(0));
        assert!(!is_positive(-1));
    }
}
'''


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
    # Clean previous (exist_ok=True handles cases where rmtree silently fails,
    # e.g., stale processes holding files open)
    shutil.rmtree(env.root, ignore_errors=True)

    env.root.mkdir(parents=True, exist_ok=True)
    env.work_base.mkdir(parents=True, exist_ok=True)
    env.repo.mkdir(parents=True, exist_ok=True)

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


def setup_claude_code_config(
    env: DemoEnv,
    worktree_paths: list[str],
    allowed_tools: list[str] = None,
) -> None:
    """Set up Claude Code configuration to skip first-run dialogs.

    Args:
        env: Demo environment
        worktree_paths: List of worktree paths to pre-approve for trust
        allowed_tools: List of tools to pre-approve (default: none, Claude will ask)
    """
    api_key_suffix = os.environ.get("ANTHROPIC_API_KEY", "")[-20:] if os.environ.get("ANTHROPIC_API_KEY") else ""

    # Build projects config - pre-approve trust for all worktree paths
    projects_config = {}
    for path in worktree_paths:
        projects_config[path] = {"allowedTools": [], "hasTrustDialogAccepted": True}

    claude_json = env.home / ".claude.json"
    claude_json.write_text(json.dumps({
        "numStartups": 100,
        "installMethod": "global",
        "theme": "light",
        "firstStartTime": "2025-01-01T00:00:00.000Z",
        "hasCompletedOnboarding": True,
        "hasCompletedClaudeInChromeOnboarding": True,
        "claudeInChromeDefaultEnabled": False,
        "sonnet45MigrationComplete": True,
        "opus45MigrationComplete": True,
        "thinkingMigrationComplete": True,
        "hasShownOpus45Notice": {},
        "lastReleaseNotesSeen": "99.0.0",
        "lastOnboardingVersion": "99.0.0",
        "oauthAccount": {
            "displayName": "wt",
            "emailAddress": "demo@example.com"
        },
        "customApiKeyResponses": {
            "approved": [api_key_suffix] if api_key_suffix else [],
            "rejected": []
        },
        "officialMarketplaceAutoInstalled": True,
        "tipsHistory": {
            "new-user-warmup": 100,
            "terminal-setup": 100,
            "theme-command": 100
        },
        "projects": projects_config
    }, indent=2))

    # Claude settings.json
    claude_dir = env.home / ".claude"
    claude_dir.mkdir(exist_ok=True)
    settings = {
        "permissions": {
            "allow": allowed_tools or [],
            "deny": [],
            "ask": []
        },
        "model": "claude-opus-4-5-20251101",
        "statusLine": {
            "type": "command",
            "command": "wt list statusline --claude-code"
        }
    }
    (claude_dir / "settings.json").write_text(json.dumps(settings, indent=2))


def setup_zellij_config(env: DemoEnv, default_cwd: str = None) -> None:
    """Set up Zellij configuration for demo recording.

    Creates config with warm-gold theme, minimal keybinds, and tab-rename plugin.
    Copies plugins from real HOME if available.

    Args:
        env: Demo environment
        default_cwd: Optional default working directory for new panes
    """
    real_zellij_plugins = REAL_HOME / ".config" / "zellij" / "plugins"

    zellij_config_dir = env.home / ".config" / "zellij"
    zellij_config_dir.mkdir(parents=True, exist_ok=True)
    zellij_plugins_dir = zellij_config_dir / "plugins"
    zellij_plugins_dir.mkdir(exist_ok=True)

    # Copy Zellij plugins from real HOME
    if real_zellij_plugins.exists():
        for plugin in real_zellij_plugins.glob("*.wasm"):
            shutil.copy(plugin, zellij_plugins_dir / plugin.name)

    default_cwd_line = f'default_cwd "{default_cwd}"' if default_cwd else ""

    zellij_config = zellij_config_dir / "config.kdl"
    zellij_config.write_text(f'''// Demo Zellij config
default_shell "fish"
{default_cwd_line}
pane_frames false
show_startup_tips false
show_release_notes false
theme "warm-gold"

// Warm gold theme to match the demo aesthetic
themes {{
    warm-gold {{
        fg "#1f2328"
        bg "#FFFDF8"
        black "#f5f0e8"
        red "#d73a49"
        green "#22863a"
        yellow "#d29922"
        blue "#0969da"
        magenta "#8250df"
        cyan "#1b7c83"
        white "#57534e"
        orange "#d97706"
    }}
}}

load_plugins {{
  "file:{zellij_plugins_dir}/zellij-tab-name.wasm"
}}

keybinds clear-defaults=true {{
    normal {{
        bind "Ctrl Space" {{ SwitchToMode "tmux"; }}
    }}
    tmux {{
        bind "p" {{ SwitchToMode "pane"; }}
        bind "t" {{ SwitchToMode "tab"; }}
        bind "q" {{ Quit; }}
    }}
    tab {{
        bind "n" {{ NewTab; SwitchToMode "Normal"; }}
        bind "h" "Left" {{ GoToPreviousTab; SwitchToMode "Normal"; }}
        bind "l" "Right" {{ GoToNextTab; SwitchToMode "Normal"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Normal"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Normal"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Normal"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Normal"; }}
    }}
    shared_except "locked" {{
        bind "Ctrl t" {{ NewTab; }}
        bind "Ctrl n" {{ NewPane; }}
    }}
    shared_except "normal" {{
        bind "Ctrl Space" "Ctrl c" {{ SwitchToMode "normal"; }}
        bind "Esc" {{ SwitchToMode "normal"; }}
    }}
}}
''')


def setup_fish_config(env: DemoEnv, repo_root: Path, wsl_create: bool = False) -> None:
    """Set up Fish shell configuration for demo recording.

    Creates config with wsl abbreviation, starship, wt shell integration,
    and Zellij tab auto-rename. Runs `wt config shell install` to install
    shell extension and completions.

    Args:
        env: Demo environment
        repo_root: Path to worktrunk repo (for wt binary)
        wsl_create: If True, wsl abbreviation includes --create flag
    """
    fish_config_dir = env.home / ".config" / "fish"
    fish_config_dir.mkdir(parents=True, exist_ok=True)

    wsl_cmd = "wt switch --execute=claude --create" if wsl_create else "wt switch --execute=claude"

    fish_config = fish_config_dir / "config.fish"
    fish_config.write_text(f'''# Demo fish config
set -U fish_greeting ""
# wsl abbreviation: switch to worktree and launch Claude
abbr --add wsl '{wsl_cmd}'
starship init fish | source

# Disable cursor blinking for VHS recording
set fish_cursor_default block
# Send escape sequences to disable cursor blink
printf '\\e[?12l'  # Disable cursor blink mode
printf '\\e[2 q'   # Set steady block cursor (non-blinking)

# Auto-rename Zellij tabs based on git branch (for demo)
function __zellij_tab_rename --on-variable PWD
    if set -q ZELLIJ
        # Get git branch name, fallback to directory basename
        set -l branch (git rev-parse --abbrev-ref HEAD 2>/dev/null)
        if test -n "$branch"
            zellij action rename-tab $branch
        end
    end
end
''')

    # Install shell extension and completions via wt command
    wt_bin = repo_root / "target" / "debug" / "wt"
    install_env = os.environ.copy()
    install_env["HOME"] = str(env.home)
    run([str(wt_bin), "config", "shell", "install", "fish", "--force"], env=install_env)


def setup_mock_clis(env: DemoEnv) -> None:
    """Set up comprehensive mock CLIs for all demo scenarios.

    Creates mocks for: npm, docker, flyctl, llm, cargo.
    Each mock handles all cases - demos just use the branches they need.
    """
    bin_dir = env.home / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)

    # npm mock - handles install, build, dev (with optional port)
    npm_mock = bin_dir / "npm"
    npm_mock.write_text("""#!/bin/bash
if [[ "$1" == "install" ]]; then
    echo "added 847 packages in 3.2s"
elif [[ "$1" == "run" && "$2" == "build" ]]; then
    echo "vite v5.4.2 building for production..."
    echo "✓ 142 modules transformed"
    echo "dist/index.js  45.2 kB │ gzip: 14.8 kB"
elif [[ "$1" == "run" && "$2" == "dev" ]]; then
    # Extract port from args if provided (e.g., npm run dev -- --port 3001)
    port=3000
    for arg in "$@"; do
        if [[ "$prev" == "--port" ]]; then
            port="$arg"
        fi
        prev="$arg"
    done
    echo ""
    echo "  VITE v5.4.2  ready in 342 ms"
    echo ""
    echo "  ➜  Local:   http://localhost:$port/"
    echo "  ➜  Network: http://192.168.1.42:$port/"
fi
""")
    npm_mock.chmod(0o755)

    # docker mock - handles compose up
    docker_mock = bin_dir / "docker"
    docker_mock.write_text("""#!/bin/bash
if [[ "$1" == "compose" && "$2" == "up" ]]; then
    echo "[+] Running 1/1"
    echo " ✔ Container postgres  Started"
fi
""")
    docker_mock.chmod(0o755)

    # flyctl mock - handles scale
    flyctl_mock = bin_dir / "flyctl"
    flyctl_mock.write_text("""#!/bin/bash
if [[ "$1" == "scale" ]]; then
    echo "Scaling app to 0 machines"
fi
""")
    flyctl_mock.chmod(0o755)

    # llm mock - simulates LLM commit message generation
    llm_mock = bin_dir / "llm"
    llm_mock.write_text("""#!/bin/bash
sleep 0.5
echo "feat(validation): add input validation utilities"
echo ""
echo "Add validation module with is_positive and is_non_empty helpers"
echo "for validating user input. Includes comprehensive test coverage."
""")
    llm_mock.chmod(0o755)

    # cargo mock - handles nextest run
    cargo_mock = bin_dir / "cargo"
    cargo_mock.write_text(r"""#!/bin/bash
if [[ "$1" == "nextest" && "$2" == "run" ]]; then
    sleep 0.3
    echo "    Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.02s"
    echo "    Starting 2 tests across 1 binary"
    echo "        PASS [   0.001s] acme::tests::test_add"
    echo "        PASS [   0.001s] acme::tests::test_add_zeros"
    echo "------------"
    echo "     Summary [   0.002s] 2 tests run: 2 passed, 0 skipped"
fi
""")
    cargo_mock.chmod(0o755)


def prepare_demo_repo(env: DemoEnv, repo_root: Path, hooks_config: str = None):
    """Set up a full demo repository with varied branches.

    Creates a rich repo for great `wt list` output:
    - Git repo with Rust project
    - Mock CLIs (npm, docker, flyctl, llm, cargo, gh)
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
    alpha, beta, hooks. Demos can then add their own config.
    """
    # Base setup: git repo, Rust project, bat wrapper, wt binary
    prepare_base_repo(env, repo_root)

    # Set up all mock CLIs - demos use what they need
    setup_mock_clis(env)

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

    # Add utils module with substantial content
    shutil.copy(FIXTURES_DIR / "alpha-utils.rs", env.repo / "src" / "utils.rs")
    # Update lib.rs to include the module
    lib_rs = env.repo / "src" / "lib.rs"
    lib_content = lib_rs.read_text()
    lib_rs.write_text("pub mod utils;\n\n" + lib_content)
    git(["-C", str(env.repo), "add", "src/utils.rs", "src/lib.rs"])
    commit_dated(env.repo, "feat: add utility functions module", "3d")

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


# =============================================================================
# Demo recording infrastructure
# =============================================================================


def check_dependencies(commands: list[str]):
    """Check that required commands are available, exit if not."""
    for cmd in commands:
        if not shutil.which(cmd):
            raise SystemExit(f"Missing dependency: {cmd}")


def setup_demo_output(out_dir: Path) -> Path:
    """Set up demo output directory and copy starship config.

    Returns the path to the starship config file.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    starship_config = out_dir / "starship.toml"
    shutil.copy(FIXTURES_DIR / "starship.toml", starship_config)
    return starship_config


def build_shell_env(demo_env: "DemoEnv", repo_root: Path, extra: dict = None) -> dict:
    """Build environment dict for running shell commands in demo context.

    Includes wt, fish, starship setup with isolated HOME.
    """
    starship_config = demo_env.out_dir / "starship.toml"
    starship_cache = demo_env.root / "starship-cache"
    starship_cache.mkdir(exist_ok=True)

    env = os.environ.copy()
    env.update({
        "LANG": "en_US.UTF-8",
        "LC_ALL": "en_US.UTF-8",
        "COLUMNS": "140",
        "RUSTUP_HOME": str(REAL_HOME / ".rustup"),
        "CARGO_HOME": str(REAL_HOME / ".cargo"),
        "HOME": str(demo_env.home),
        "PATH": f"{repo_root}/target/debug:{demo_env.home}/bin:{os.environ['PATH']}",
        "STARSHIP_CONFIG": str(starship_config),
        "STARSHIP_CACHE": str(starship_cache),
        "NO_COLOR": "1",
        "CLICOLOR": "0",
    })
    if extra:
        env.update(extra)
    return env


def clean_ansi_output(text: str) -> str:
    """Strip ANSI escape codes and control characters from text."""
    # Strip ANSI escape sequences
    clean = re.sub(r"\x1B\[[0-9;?]*[A-Za-z]", "", text)
    # Strip control characters (except newline, tab, carriage return)
    clean = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", "", clean)
    return clean


def run_fish_script(
    demo_env: "DemoEnv",
    script: str,
    env: dict,
    cwd: Path = None,
) -> str:
    """Run a fish script and return cleaned output.

    Automatically prepends shell init and cleans ANSI from output.
    """
    full_script = "wt config shell init fish | source\n" + script
    result = subprocess.run(
        ["fish", "-c", full_script],
        cwd=cwd or demo_env.repo,
        env=env,
        capture_output=True,
        text=True,
    )
    return clean_ansi_output(result.stdout + result.stderr)


@dataclass
class DemoSize:
    """Canvas and font size for demo recording."""
    width: int
    height: int
    fontsize: int


# Predefined sizes for different contexts
SIZE_TWITTER = DemoSize(width=1200, height=700, fontsize=26)  # Big text for mobile
SIZE_DOCS = DemoSize(width=1600, height=900, fontsize=24)     # More content for docs


def record_all_themes(
    demo_env: "DemoEnv",
    tape_template: Path,
    output_gifs: dict[str, Path],
    repo_root: Path,
    vhs_binary: str = "vhs",
    size: DemoSize = None,
):
    """Record demo GIFs for all themes.

    Args:
        demo_env: Demo environment with repo and home paths
        tape_template: Path to the .tape template file
        output_gifs: Dict of theme_name -> output GIF path (e.g., {"light": path, "dark": path})
        repo_root: Path to worktrunk repo root (for target/debug)
        vhs_binary: VHS binary to use (default "vhs", can be path to custom build)
        size: Canvas and font size (default SIZE_DOCS)
    """
    if size is None:
        size = SIZE_DOCS

    tape_rendered = demo_env.out_dir / ".rendered.tape"
    starship_config = demo_env.out_dir / "starship.toml"
    docs_assets = repo_root / "docs" / "static" / "assets"
    docs_assets.mkdir(parents=True, exist_ok=True)

    for theme_name, output_gif in output_gifs.items():
        theme = THEMES[theme_name]
        replacements = {
            "DEMO_REPO": demo_env.repo,
            "DEMO_HOME": demo_env.home,
            "REAL_HOME": REAL_HOME,
            "STARSHIP_CONFIG": starship_config,
            "OUTPUT_GIF": output_gif,
            "TARGET_DEBUG": repo_root / "target" / "debug",
            "THEME": format_theme_for_vhs(theme),
            "ANTHROPIC_API_KEY": os.environ.get("ANTHROPIC_API_KEY", ""),
            "WIDTH": size.width,
            "HEIGHT": size.height,
            "FONTSIZE": size.fontsize,
        }

        if not render_tape(tape_template, tape_rendered, replacements):
            continue

        print(f"\nRecording {theme_name} GIF...")
        record_vhs(tape_rendered, vhs_binary)
        tape_rendered.unlink(missing_ok=True)
        print(f"GIF saved to {output_gif}")

        # Copy to docs for local preview
        shutil.copy(output_gif, docs_assets / output_gif.name)
        print(f"Copied to {docs_assets / output_gif.name}")
