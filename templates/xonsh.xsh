# worktrunk shell integration for xonsh

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
import shutil
import os
import sys
if shutil.which("{{ cmd_prefix }}") is not None or os.environ.get('WORKTRUNK_BIN'):
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: $WORKTRUNK_BIN = ./target/debug/{{ cmd_prefix }}
    _WORKTRUNK_CMD = os.environ.get('WORKTRUNK_BIN', '{{ cmd_prefix }}')

    def wt_exec(args, cmd=None):
        """Helper function to parse wt output and handle directives
        Directives are NUL-terminated to support multi-line commands"""
        import subprocess

        # Use provided command or default to _WORKTRUNK_CMD
        command = cmd if cmd is not None else _WORKTRUNK_CMD

        # Start process with stdout redirection for real-time streaming
        # stderr passes through to terminal for TTY detection
        proc = subprocess.Popen(
            [command] + args,
            stdout=subprocess.PIPE,
            stderr=None,  # Let stderr pass through
            text=True,
            bufsize=0  # Unbuffered for real-time streaming
        )

        # Read stdout character by character for real-time processing
        buffer = ""
        exec_cmd = ""

        # Read and process output as it arrives
        try:
            while True:
                char = proc.stdout.read(1)
                if not char:
                    break

                if char == '\0':
                    # NUL byte - process the buffered chunk immediately
                    if buffer.startswith("__WORKTRUNK_CD__"):
                        # CD directive - extract path and change directory
                        path = buffer.replace("__WORKTRUNK_CD__", "", 1)
                        if os.path.isdir(path):
                            cd @(path)
                        else:
                            print(f"Error: Not a directory: {path}", file=sys.stderr)
                    elif buffer.startswith("__WORKTRUNK_EXEC__"):
                        # EXEC directive - extract command (may contain newlines)
                        exec_cmd = buffer.replace("__WORKTRUNK_EXEC__", "", 1)
                    elif buffer:
                        # Regular output - print it immediately
                        print(buffer)

                    buffer = ""
                else:
                    buffer += char
        except IOError as e:
            print(f"Error reading command output: {e}", file=sys.stderr)

        # Process any remaining buffer (shouldn't happen with NUL-terminated protocol)
        if buffer:
            print(buffer)

        # Wait for process and get actual exit code
        exit_code = proc.wait()

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if exec_cmd:
            # Security: Command comes from wt --internal output; eval is intentional
            execx(exec_cmd)
            # execx() sets __xonsh__.env['LASTRETURNCODE'] to the exit code
            return __xonsh__.env.get('LASTRETURNCODE', 0)

        # Return the actual exit code from the command
        return exit_code

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
            cmd = "./target/debug/{{ cmd_prefix }}"
        else:
            cmd = _WORKTRUNK_CMD

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if 'NO_COLOR' not in os.environ and 'CLICOLOR_FORCE' not in os.environ:
            if sys.stdout.isatty():
                os.environ['CLICOLOR_FORCE'] = '1'

        # Always use --internal mode for directive support
        return wt_exec(["--internal"] + filtered_args, cmd=cmd)

    # Register the alias
    aliases['{{ cmd_prefix }}'] = _{{ cmd_prefix }}_wrapper
