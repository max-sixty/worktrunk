# worktrunk shell integration for PowerShell

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if ((Get-Command wt -ErrorAction SilentlyContinue) -or $env:WORKTRUNK_BIN) {
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: $env:WORKTRUNK_BIN = "./target/debug/wt"
    $script:_WORKTRUNK_CMD = if ($env:WORKTRUNK_BIN) { $env:WORKTRUNK_BIN } else { "wt" }

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    function _wt_exec {
        param(
            [string]$Command,
            [Parameter(ValueFromRemainingArguments=$true)]
            [string[]]$Arguments
        )

        # Use provided command or default to _WORKTRUNK_CMD
        $cmd = if ($Command) { $Command } else { $script:_WORKTRUNK_CMD }

        # Capture stdout for directives, let stderr pass through to terminal
        # This preserves TTY for color detection
        $output = & $cmd @Arguments | Out-String
        $exitCode = $LASTEXITCODE
        $execCmd = ""

        # Split output on NUL bytes, process each chunk
        foreach ($chunk in ($output -split "`0")) {
            if ($chunk -match '^__WORKTRUNK_CD__') {
                # CD directive - extract path and change directory
                $path = $chunk -replace '^__WORKTRUNK_CD__', ''
                Set-Location $path
            } elseif ($chunk -match '^__WORKTRUNK_EXEC__') {
                # EXEC directive - extract command (may contain newlines)
                $execCmd = $chunk -replace '^__WORKTRUNK_EXEC__', ''
            } elseif ($chunk) {
                # Regular output - write it with newline
                Write-Output $chunk
            }
        }

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if ($execCmd) {
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
            $cmd = "./target/debug/wt"
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
        $exitCode = _wt_exec -Command $cmd --internal @filteredArgs
        return $exitCode
    }
}
