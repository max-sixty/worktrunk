# worktrunk shell integration for PowerShell

# Helper function to parse wt output and handle directives
function _wt_exec {
    param(
        [Parameter(ValueFromRemainingArguments=$true)]
        [string[]]$Arguments
    )

    # Capture output and exit code
    $output = & wt @Arguments 2>&1
    $exitCode = $LASTEXITCODE

    # Parse output line by line
    foreach ($line in $output) {
        if ($line -match '^__WORKTRUNK_CD__(.+)$') {
            # Extract path and change directory
            Set-Location $matches[1]
        } else {
            # Regular output - write it
            Write-Output $line
        }
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
        & wt
        return $LASTEXITCODE
    }

    $subcommand = $Arguments[0]

    switch ($subcommand) {
        { $_ -in @("switch", "remove", "merge") } {
            # Commands that need --internal for directory change support
            $restArgs = $Arguments[1..($Arguments.Count-1)]
            $exitCode = _wt_exec $subcommand --internal @restArgs
            return $exitCode
        }
        default {
            # All other commands pass through directly
            & wt @Arguments
            return $LASTEXITCODE
        }
    }
}
