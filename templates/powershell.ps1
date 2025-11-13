# worktrunk shell integration for PowerShell

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if ((Get-Command {{ cmd_prefix }} -ErrorAction SilentlyContinue) -or $env:WORKTRUNK_BIN) {
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: $env:WORKTRUNK_BIN = "./target/debug/{{ cmd_prefix }}"
    $script:_WORKTRUNK_CMD = if ($env:WORKTRUNK_BIN) { $env:WORKTRUNK_BIN } else { "{{ cmd_prefix }}" }

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    function wt_exec {
        param(
            [string]$Command,
            [Parameter(ValueFromRemainingArguments=$true)]
            [string[]]$Arguments
        )

        # Use provided command or default to _WORKTRUNK_CMD
        $cmd = if ($Command) { $Command } else { $script:_WORKTRUNK_CMD }

        # Set up process with stdout redirection for real-time streaming
        # stderr passes through to terminal for TTY detection
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $cmd
        $psi.Arguments = ($Arguments | ForEach-Object {
            if ($_ -match '\s') { "`"$_`"" } else { $_ }
        }) -join ' '
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $false
        $psi.UseShellExecute = $false

        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $psi
        $process.Start() | Out-Null

        # Read stdout character by character for real-time processing
        $reader = $process.StandardOutput
        $buffer = New-Object System.Text.StringBuilder
        $execCmd = ""

        # Read and process output as it arrives
        while (-not $reader.EndOfStream) {
            $charCode = $reader.Read()
            if ($charCode -eq -1) { break }  # Explicit EOF check
            $char = [char]$charCode

            if ($char -eq "`0") {
                # NUL byte - process the buffered chunk immediately
                $chunk = $buffer.ToString()

                if ($chunk -match '^__WORKTRUNK_CD__') {
                    # CD directive - extract path and change directory
                    $path = $chunk -replace '^__WORKTRUNK_CD__', ''
                    if (Test-Path -Path $path -PathType Container) {
                        Set-Location $path
                    } else {
                        Write-Error "Error: Not a directory: $path"
                    }
                } elseif ($chunk -match '^__WORKTRUNK_EXEC__') {
                    # EXEC directive - extract command (may contain newlines)
                    $execCmd = $chunk -replace '^__WORKTRUNK_EXEC__', ''
                } elseif ($chunk) {
                    # Regular output - write it immediately
                    Write-Output $chunk
                }

                $buffer.Clear()
            } else {
                $buffer.Append($char) | Out-Null
            }
        }

        # Process any remaining buffer (shouldn't happen with NUL-terminated protocol)
        if ($buffer.Length -gt 0) {
            $chunk = $buffer.ToString()
            if ($chunk) {
                Write-Output $chunk
            }
        }

        # Wait for process and get actual exit code
        $process.WaitForExit()
        $exitCode = $process.ExitCode

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if ($execCmd) {
            # Security: Command comes from wt --internal output; eval is intentional
            Invoke-Expression $execCmd
            $exitCode = $LASTEXITCODE
        }

        # Return the exit code
        return $exitCode
    }

    # Override {{ cmd_prefix }} command to add --internal flag
    function {{ cmd_prefix }} {
        param(
            [Parameter(ValueFromRemainingArguments=$true)]
            [string[]]$Arguments
        )

        $useSource = $false
        $filteredArgs = @()

        # Check for --source flag and strip it
        foreach ($arg in $Arguments) {
            if ($arg -eq "--source") {
                $useSource = $true
            } else {
                $filteredArgs += $arg
            }
        }

        # Determine which command to use
        if ($useSource) {
            # Build the project
            cargo build --quiet 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Error "Error: cargo build failed"
                return 1
            }
            $cmd = "./target/debug/{{ cmd_prefix }}"
        } else {
            $cmd = $script:_WORKTRUNK_CMD
        }

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if (-not $env:NO_COLOR -and -not $env:CLICOLOR_FORCE) {
            if ([Console]::IsOutputRedirected -eq $false) {
                $env:CLICOLOR_FORCE = "1"
            }
        }

        # Always use --internal mode for directive support
        $exitCode = wt_exec -Command $cmd --internal @filteredArgs
        return $exitCode
    }
}
