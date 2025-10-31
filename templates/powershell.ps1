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
                # TODO: Use Write-Output instead of Write-Host for redirectable output
                Write-Host $chunk
            }
        }

        # Execute command if one was specified
        if ($execCmd) {
            Invoke-Expression $execCmd
        }

        # Return the exit code
        return $exitCode
    }

    # Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
    function {{ cmd_prefix }} {
        param(
            [Parameter(ValueFromRemainingArguments=$true)]
            [string[]]$Arguments
        )

        $useDev = $false
        $filteredArgs = @()

        # Check for --dev flag and strip it
        foreach ($arg in $Arguments) {
            if ($arg -eq "--dev") {
                $useDev = $true
            } else {
                $filteredArgs += $arg
            }
        }

        # Determine which command to use
        if ($useDev) {
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

        if ($filteredArgs.Count -eq 0) {
            & $cmd
            return $LASTEXITCODE
        }

        $subcommand = $filteredArgs[0]

        switch ($subcommand) {
            { $_ -in @("switch", "remove", "merge") } {
                # Commands that need --internal for directory change support
                $restArgs = $filteredArgs[1..($filteredArgs.Count-1)]
                $exitCode = _wt_exec -Command $cmd --internal $subcommand @restArgs
                return $exitCode
            }
            default {
                # All other commands pass through directly
                & $cmd @filteredArgs
                return $LASTEXITCODE
            }
        }
    }
}
