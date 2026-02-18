# worktrunk shell integration for nushell

# Override {{ cmd }} command with file-based directive passing.
# Creates a temp file, passes path via WORKTRUNK_DIRECTIVE_FILE, executes directives after.
# WORKTRUNK_BIN can override the binary path (for testing dev builds).
#
# Note: Nushell's `source` is parse-time only, so we can't source dynamic paths.
# Instead, we read the directive file and execute each line.
#
# Nushell pipeline workaround (nushell/nushell#12643):
#
# In POSIX shells, pipes are file-descriptor plumbing — stdout flows through
# the pipe while the shell continues executing post-command logic (directive
# processing, exit codes). In nushell, a function's "output" is its return
# value (the last expression), not bytes written to fd1. When an external
# command isn't the last expression, its stdout goes to the terminal as a
# side effect but doesn't flow through `|`.
#
# This means we can't both stream stdout through a pipe AND do post-processing
# (directives, cleanup) in the same function. We handle this by splitting into
# two paths based on the subcommand:
#
# - `list`: Direct passthrough. The binary is the last expression, so stdout
#   streams to the terminal (progressive rendering) and flows through pipes
#   (`{{ cmd }} list --format json | from json`). No directive processing — `list`
#   never emits directives. No WORKTRUNK_DIRECTIVE_FILE is set.
#
# - Everything else: Stdout is redirected to a temp file via `o>`, then
#   returned as the function's last expression after directive processing.
#   Stderr flows to the terminal in real-time. The binary sees non-TTY stdout
#   and uses buffered mode, but commands other than `list` don't benefit from
#   progressive rendering anyway.
def --env --wrapped {{ cmd }} [...args: string] {
    let worktrunk_bin = if ($env.WORKTRUNK_BIN? | is-not-empty) {
        $env.WORKTRUNK_BIN
    } else {
        # Find the external binary, not the custom function
        # `which -a` is needed because `which` alone only returns the custom command
        let external = (which -a {{ cmd }} | where type == "external")
        if ($external | is-empty) {
            error make {
                msg: "{{ cmd }} binary not found in PATH. Install with 'cargo install --path .' or set $env.WORKTRUNK_BIN"
            }
        }
        ($external | get 0.path)
    }

    # `list` is the only command that benefits from streaming stdout (progressive
    # table rendering). It never emits directives, so we skip directive processing
    # and let the binary be the last expression — stdout flows through pipes.
    # Note: global flags before `list` (e.g. `wt -C /path list`) miss this check
    # and fall through to the buffered path. Output is still correct, just not
    # progressively rendered.
    if (not ($args | is-empty)) and ($args | first) == "list" {
        # Direct passthrough: binary is the last expression of this branch,
        # which is the last expression of the function. Stdout streams to the
        # terminal and flows through nushell pipelines.
        ^$worktrunk_bin ...$args
    } else {
        let directive_file = (mktemp --tmpdir)
        let stdout_file = (mktemp --tmpdir)

        # Capture stdout to file for pipeline passthrough; stderr flows to terminal.
        # Nushell 0.98+ throws ShellError on non-zero exit (like bash `set -e`).
        # `try` catches it so directive processing and temp file cleanup still run.
        let exit_code = (try {
            with-env { WORKTRUNK_DIRECTIVE_FILE: $directive_file } {
                ^$worktrunk_bin ...$args o> $stdout_file
            }
            0
        } catch {
            $env.LAST_EXIT_CODE
        })

        # Process directive file line by line
        if ($directive_file | path exists) and (open $directive_file --raw | str trim | is-not-empty) {
            let directives = open $directive_file --raw | str trim | lines
            for directive in $directives {
                if ($directive | str starts-with "cd '") {
                    # Parse cd command: worktrunk emits "cd '<path>'" with POSIX quoting.
                    # Strip the "cd '" prefix and trailing "'" to extract the raw path.
                    # Note: paths with embedded single quotes use POSIX '\'' escaping,
                    # which this doesn't handle. A future improvement could have worktrunk
                    # emit raw paths instead of POSIX shell commands, letting each shell
                    # construct its native cd syntax.
                    let target_dir = $directive | str substring 4..-2
                    cd $target_dir
                } else if ($directive | is-not-empty) {
                    # Execute via sh for POSIX shell expansion (globs, pipes, $VAR).
                    # --execute commands are user-provided shell expressions that expect
                    # this. Env changes (export) won't persist in the nushell session,
                    # but no worktrunk code emits export directives.
                    ^sh -c $directive
                }
            }
        }

        rm -f $directive_file
        let output = (open $stdout_file --raw)
        rm -f $stdout_file

        # Return stdout or propagate failure as the function's last expression.
        # Using a failing external command (not `error make`) so nushell treats it
        # identically to the original non-zero exit — minimal display in scripts,
        # no verbose source trace.
        if $exit_code != 0 {
            if ($output | is-not-empty) { print -n $output }
            ^sh -c $"exit ($exit_code)"
        } else if ($output | is-not-empty) {
            $output
        }
    }
}
