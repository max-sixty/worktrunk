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
                    path = chunk.replace("__WORKTRUNK_CD__", "", 1)
                    cd @(path)
                elif chunk.startswith("__WORKTRUNK_EXEC__"):
                    # EXEC directive - extract command (may contain newlines)
                    exec_cmd = chunk.replace("__WORKTRUNK_EXEC__", "", 1)
                elif chunk:
                    # Regular output - print it with newline
                    print(chunk)

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if exec_cmd:
            execx(exec_cmd)
            # execx() sets __xonsh__.env['LASTRETURNCODE'] to the exit code
            return __xonsh__.env.get('LASTRETURNCODE', 0)

        # Return the exit code, defaulting to 0 if the subprocess did not set one
        return result.returncode if result.returncode is not None else 0

    def _{{ cmd_prefix }}_wrapper(args):
        """Override {{ cmd_prefix }} command to add --internal flag"""
        use_source = False
        filtered_args = []

        # Check for --source flag and strip it
        for arg in args:
            if arg == "--source":
                use_source = True
            else:
                filtered_args.append(arg)

        # Determine which command to use
        if use_source:
            # Build the project
            build_result = !(cargo build --quiet)
            if build_result.returncode != 0:
                print("Error: cargo build failed", file=sys.stderr)
                return 1
            cmd = "./target/debug/wt"
        else:
            cmd = _WORKTRUNK_CMD

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if 'NO_COLOR' not in os.environ and 'CLICOLOR_FORCE' not in os.environ:
            if sys.stdout.isatty():
                os.environ['CLICOLOR_FORCE'] = '1'

        # Always use --internal mode for directive support
        return _wt_exec(["--internal"] + filtered_args, cmd=cmd)

    # Register the alias
    aliases['{{ cmd_prefix }}'] = _{{ cmd_prefix }}_wrapper
