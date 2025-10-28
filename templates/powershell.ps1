# worktrunk shell integration for PowerShell

# Only initialize if wt is available
if (Get-Command wt -ErrorAction SilentlyContinue) {
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: $env:WORKTRUNK_BIN = "./target/debug/wt"
    $script:_WORKTRUNK_CMD = if ($env:WORKTRUNK_BIN) { $env:WORKTRUNK_BIN } else { "wt" }

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    function _wt_exec {
        param(
            [Parameter(ValueFromRemainingArguments=$true)]
            [string[]]$Arguments
        )

        # Capture stdout for directives, let stderr pass through to terminal
        # This preserves TTY for color detection
        $output = & $script:_WORKTRUNK_CMD @Arguments | Out-String
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
                # Regular output - write it (preserving newlines)
                # TODO: Use Write-Output instead of Write-Host for redirectable output
                Write-Host -NoNewline $chunk
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

        if ($Arguments.Count -eq 0) {
            & $script:_WORKTRUNK_CMD
            return $LASTEXITCODE
        }

        $subcommand = $Arguments[0]

        switch ($subcommand) {
            { $_ -in @("switch", "remove", "merge") } {
                # Commands that need --internal for directory change support
                $restArgs = $Arguments[1..($Arguments.Count-1)]
                $exitCode = _wt_exec --internal $subcommand @restArgs
                return $exitCode
            }
            default {
                # All other commands pass through directly
                & $script:_WORKTRUNK_CMD @Arguments
                return $LASTEXITCODE
            }
        }
    }
}
