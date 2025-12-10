# Intermittent Ctrl-C (SIGINT) Not Working Through Shell Wrapper

## Problem Summary

When running `cargo nextest` through worktrunk's shell wrapper (via `wt hook pre-merge`), Ctrl-C occasionally fails to terminate the running tests. The issue is intermittent — sometimes Ctrl-C works correctly, sometimes it appears to be ignored.

## Goals

1. Understand whether worktrunk's shell wrapping is causing the signal delivery issue
2. Determine if the same behavior would occur running `cargo nextest` directly
3. Identify the root cause and potential fixes

## System Context

- **Platform**: macOS (Darwin 25.0.0)
- **Shell**: `/bin/sh -c` (POSIX shell)
- **Tool chain**: Rust CLI tool (`wt`) executing shell commands

## Architecture Overview

### Process Tree When Running `wt hook pre-merge`

```
wt (worktrunk binary)
└── sh -c "{ cargo nextest run ...\n} 1>&2"
    └── cargo nextest run ...
        └── test runner process
            └── individual test processes
```

### How Commands Are Executed

Worktrunk uses a shell wrapper pattern to execute hook commands. The relevant code flow is:

1. `wt hook pre-merge` calls `execute_command_in_worktree()`
2. This calls `execute_streaming()` with `redirect_stdout_to_stderr=true`
3. `execute_streaming()` wraps the command and spawns via `/bin/sh -c`

### Key Code: Shell Execution (`src/output/handlers.rs`)

```rust
pub(crate) fn execute_streaming(
    command: &str,
    working_dir: &std::path::Path,
    redirect_stdout_to_stderr: bool,
    stdin_content: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::Write;
    use worktrunk::git::WorktrunkError;
    use worktrunk::shell_exec::ShellConfig;

    let shell = ShellConfig::get();

    // Determine stdout handling based on shell and redirect flag
    let (command_to_run, stdout_mode) = if redirect_stdout_to_stderr {
        if shell.is_posix() {
            // POSIX: wrap command to redirect stdout to stderr at shell level
            // Use newline instead of semicolon before closing brace to support
            // multi-line commands with control structures (if/fi, for/done, etc.)
            (
                format!("{{ {}\n}} 1>&2", command),
                std::process::Stdio::inherit(),
            )
        } else {
            // Non-POSIX (PowerShell): redirect stdout to stderr at OS level
            (
                command.to_string(),
                std::process::Stdio::from(std::io::stderr()),
            )
        }
    } else {
        (command.to_string(), std::process::Stdio::inherit())
    };

    let stdin_mode = if stdin_content.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    };

    let mut cmd = shell.command(&command_to_run);
    let mut child = cmd
        .current_dir(working_dir)
        .stdin(stdin_mode)
        .stdout(stdout_mode)
        .stderr(std::process::Stdio::inherit()) // Preserve TTY for errors
        .env_remove("VERGEN_GIT_DESCRIBE")
        .spawn()
        .map_err(|e| {
            anyhow::Error::from(worktrunk::git::GitError::Other {
                message: format!("Failed to execute command with {}: {}", shell.name, e),
            })
        })?;

    // Write stdin content if provided (used for hook context JSON)
    if let Some(content) = stdin_content
        && let Some(mut stdin) = child.stdin.take()
    {
        // Write and close stdin immediately so the child doesn't block waiting for more input
        let _ = stdin.write_all(content.as_bytes());
        // stdin is dropped here, closing the pipe
    }

    // Wait for command to complete
    let status = child.wait().map_err(|e| {
        anyhow::Error::from(worktrunk::git::GitError::Other {
            message: format!("Failed to wait for command: {}", e),
        })
    })?;

    // Check if child was killed by a signal (Unix only)
    // This handles Ctrl-C: when SIGINT is sent, the child receives it and terminates,
    // and we propagate the signal exit code (128 + signal number, e.g., 130 for SIGINT)
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {}", sig),
        }
        .into());
    }

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(WorktrunkError::ChildProcessExited {
            code,
            message: format!("exit status: {}", code),
        }
        .into());
    }

    Ok(())
}
```

### Key Code: Shell Configuration (`src/shell_exec.rs`)

```rust
fn detect_shell() -> ShellConfig {
    #[cfg(unix)]
    {
        ShellConfig {
            executable: PathBuf::from("sh"),
            args: vec!["-c".to_string()],
            is_posix: true,
            name: "sh".to_string(),
        }
    }
}

impl ShellConfig {
    pub fn command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(&self.executable);
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(shell_command);
        cmd
    }
}
```

## What We Know

### Signal Flow (Expected Behavior)

1. User presses Ctrl-C
2. Terminal sends SIGINT to the **foreground process group**
3. All processes in that group should receive SIGINT:
   - `wt` (blocking in `child.wait()`)
   - `sh` (waiting for its child)
   - `cargo nextest` (running tests)
   - Individual test processes
4. Each process handles SIGINT (usually terminates)
5. Exit codes propagate back up

### Stdio Configuration

- **stdin**: `Stdio::piped()` when stdin_content is provided (hook JSON context), otherwise `Stdio::null()`
- **stdout**: `Stdio::inherit()` (shares parent's stdout/TTY)
- **stderr**: `Stdio::inherit()` (shares parent's stderr/TTY)

### The Shell Wrapper Pattern

The command transformation for stdout→stderr redirection:

```
Input:  cargo nextest run --all-targets
Output: { cargo nextest run --all-targets
} 1>&2
```

This gets executed as:
```bash
sh -c '{ cargo nextest run --all-targets
} 1>&2'
```

### Process Group Considerations

- By default, child processes inherit the parent's process group
- `sh -c` does NOT create a new process group
- With `Stdio::inherit()`, child processes share the controlling terminal
- SIGINT from Ctrl-C goes to the foreground process group

### What nextest Does

Nextest is known to create separate process groups for test isolation:
> "Each test runs in its own process. By default, nextest creates a new process group for each test to isolate tests from each other."

This means:
- `cargo nextest` receives SIGINT
- Individual test processes may be in different process groups
- Nextest must forward signals to its children

## Hypotheses

### Hypothesis 1: The `sh -c` Wrapper Interferes

The `{ ... } 1>&2` wrapper requires `sh` to stay resident (it can't `exec` directly because it needs to handle the redirection). This adds an extra process layer.

**Potential issue**: When `sh` is running a compound command, it waits for the inner command. If the inner command doesn't die from SIGINT (or handles it specially), `sh` keeps waiting.

### Hypothesis 2: Nextest's Process Groups

Nextest creates new process groups for test isolation. When SIGINT arrives:
1. SIGINT goes to the foreground process group (wt, sh, cargo-nextest)
2. But nextest's test child processes are in different process groups
3. Nextest's signal forwarding may race or fail depending on timing

### Hypothesis 3: No Signal Handler in Worktrunk

Worktrunk does NOT install any signal handlers. It relies entirely on:
1. The OS default behavior (SIGINT terminates the process)
2. `Stdio::inherit()` to let children receive signals from the TTY
3. Checking `ExitStatusExt::signal()` after `wait()` returns

This means while worktrunk is blocked in `child.wait()`:
- SIGINT arrives at worktrunk
- Default handler would terminate worktrunk
- But worktrunk might terminate before the child properly handles the signal

### Hypothesis 4: Stdin Pipe Timing

When `stdin_content` is provided:
1. stdin is `Stdio::piped()`
2. We write JSON content to the pipe
3. We close the pipe (drop the handle)
4. Then we `wait()`

If SIGINT arrives during the brief window where we're writing to stdin, there could be unexpected behavior.

## Open Questions

1. **Does the issue reproduce without worktrunk?**
   - Test: `sh -c '{ cargo nextest run --all-targets\n} 1>&2'`
   - If this also fails to respond to Ctrl-C, the issue is not worktrunk-specific

2. **Does the issue reproduce without the shell wrapper?**
   - Test: `cargo nextest run --all-targets` directly
   - If this works correctly, the `sh -c` wrapper is contributing

3. **What shell is `/bin/sh` on macOS?**
   - On modern macOS, `/bin/sh` is actually `zsh` in POSIX mode (or `bash` on older versions)
   - Different shells may handle signals in compound commands differently

4. **Does nextest have known signal handling issues?**
   - Are there GitHub issues about Ctrl-C not working with nextest?
   - Does nextest's `--no-capture` mode affect signal handling?

5. **Would installing a SIGINT handler in worktrunk help?**
   - Could explicitly forward SIGINT to the child process group
   - But this adds complexity and may interfere with other signal handling

6. **Does the `1>&2` redirection affect signal handling?**
   - The shell must stay resident to handle redirection
   - Does this change how the shell handles signals?

7. **Is there a timing component?**
   - Does the issue occur more often at certain points (test startup, between tests, during output)?
   - Could this be a race condition in nextest's signal forwarding?

## Research Questions for External Sources

1. **POSIX signal handling in shell compound commands**: How does `sh` handle SIGINT when running `{ cmd; } 1>&2`? Does it forward signals to the child? Does it wait for the child to exit first?

2. **Rust `std::process::Command` and signals**: When using `Stdio::inherit()`, how are signals delivered to child processes? Does the child receive SIGINT directly from the terminal?

3. **Process groups with `sh -c`**: Does `sh -c "command"` create a new process group? How does this interact with job control and signal delivery?

4. **cargo-nextest signal handling**: How does nextest handle SIGINT? Does it use process groups for test isolation? Are there known issues with signal forwarding?

5. **macOS `/bin/sh` behavior**: What shell is `/bin/sh` on modern macOS? How does it handle signals differently from Linux `dash` or `bash`?

6. **Best practices for CLI tools executing subprocesses**: What's the recommended way for a Rust CLI to execute shell commands while preserving proper Ctrl-C handling?

## Potential Solutions to Explore

1. **Avoid the shell wrapper**: Find an alternative to `{ cmd } 1>&2` that doesn't require `sh` to stay resident (e.g., use `Stdio::from(io::stderr())` for stdout like we do for non-POSIX shells)

2. **Install a signal handler**: Use the `ctrlc` crate or `signal-hook` to catch SIGINT and explicitly forward it to children

3. **Use `exec` in the shell command**: Change to `exec sh -c 'cmd'` or similar to reduce process layers

4. **Process group management**: Explicitly put the child in our process group using `setpgid`

5. **Document the limitation**: If this is a nextest issue, document it and suggest workarounds

## Test Commands

To help diagnose the issue, these commands can be compared:

```bash
# Direct execution (baseline)
cargo nextest run --all-targets

# With sh wrapper (matches worktrunk's pattern)
sh -c '{ cargo nextest run --all-targets
} 1>&2'

# Without redirection (simpler wrapper)
sh -c 'cargo nextest run --all-targets'

# With exec (reduces process layers)
sh -c 'exec cargo nextest run --all-targets'
```

Press Ctrl-C during each and observe whether termination is immediate or delayed/ignored.
