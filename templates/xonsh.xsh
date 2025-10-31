# worktrunk shell integration for xonsh

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
import shutil
import os
import sys
if shutil.which("wt") is not None or os.environ.get('WORKTRUNK_BIN'):
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: $WORKTRUNK_BIN = ./target/debug/wt
    _WORKTRUNK_CMD = os.environ.get('WORKTRUNK_BIN', 'wt')

    def _wt_exec(args, cmd=None):
        """Helper function to parse wt output and handle directives
        Directives are NUL-terminated to support multi-line commands"""
        # Use provided command or default to _WORKTRUNK_CMD
        command = cmd if cmd is not None else _WORKTRUNK_CMD
        # Capture full output including return code
        result = ![@(command) @(args)]
        exec_cmd = ""

        # Split output on NUL bytes, process each chunk
        if result.out:
            for chunk in result.out.split("\0"):
                if chunk.startswith("__WORKTRUNK_CD__"):
                    # CD directive - extract path and change directory
                    # TODO: Use str.replace instead of hard-coded offset (fragile if prefix changes)
                    path = chunk[16:]  # Remove prefix
                    cd @(path)
                elif chunk.startswith("__WORKTRUNK_EXEC__"):
                    # EXEC directive - extract command (may contain newlines)
                    # TODO: Use str.replace instead of hard-coded offset (fragile if prefix changes)
                    exec_cmd = chunk[18:]  # Remove prefix
                elif chunk:
                    # Regular output - print it with newline
                    print(chunk)

        # Execute command if one was specified
        if exec_cmd:
            execx(exec_cmd)

        # Return the exit code
        # TODO: Add fallback for None: return result.returncode or 0
        return result.returncode

    def _{{ cmd_prefix }}_wrapper(args):
        """Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge"""
        use_dev = False
        filtered_args = []

        # Check for --dev flag and strip it
        for arg in args:
            if arg == "--dev":
                use_dev = True
            else:
                filtered_args.append(arg)

        # Determine which command to use
        if use_dev:
            # Build the project
            build_result = !(cargo build --quiet)
            if build_result.returncode != 0:
                print("Error: cargo build failed", file=sys.stderr)
                return 1
            cmd = "./target/debug/wt"
        else:
            cmd = _WORKTRUNK_CMD

        if not filtered_args:
            # No arguments, just run the command
            ![@(cmd)]
            return

        subcommand = filtered_args[0]

        if subcommand in ["switch", "remove", "merge"]:
            # Commands that need --internal for directory change support
            rest_args = filtered_args[1:]
            return _wt_exec(["--internal", subcommand] + rest_args, cmd=cmd)
        else:
            # All other commands pass through directly
            result = ![@(cmd) @(filtered_args)]
            return result.returncode

    # Register the alias
    aliases['{{ cmd_prefix }}'] = _{{ cmd_prefix }}_wrapper
