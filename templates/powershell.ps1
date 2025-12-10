# worktrunk shell integration for PowerShell
#
# Limitations compared to bash/zsh/fish:
# - Hooks that use bash syntax won't work without Git Bash
#
# For full hook support on Windows, use Git Bash with `wt config shell install bash`.

# Only initialize if wt is available
if (Get-Command {{ cmd_prefix }} -ErrorAction SilentlyContinue) {

    # wt wrapper function - captures stdout and executes as PowerShell
    function {{ cmd_prefix }} {
        param(
            [Parameter(ValueFromRemainingArguments = $true)]
            [string[]]$Arguments
        )

        $wtBin = (Get-Command {{ cmd_prefix }} -CommandType Application).Source

        # Run wt with --internal=powershell, capture stdout for Invoke-Expression
        # stderr passes through to console in real-time
        $script = & $wtBin --internal=powershell @Arguments 2>&1 | Out-String

        # Execute the directive script (e.g., Set-Location) if command succeeded
        if ($LASTEXITCODE -eq 0 -and $script.Trim()) {
            Invoke-Expression $script
        }

        return $LASTEXITCODE
    }

    # Tab completion - generate clap's completer script and eval it
    # This registers Register-ArgumentCompleter with proper handling
    $env:COMPLETE = "powershell"
    try {
        & (Get-Command {{ cmd_prefix }} -CommandType Application) | Out-String | Invoke-Expression
    }
    finally {
        Remove-Item Env:\COMPLETE -ErrorAction SilentlyContinue
    }
}
