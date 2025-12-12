# worktrunk shell integration for PowerShell
#
# Limitations compared to bash/zsh/fish:
# - Hooks using bash syntax won't work without Git Bash
#
# For full hook compatibility on Windows, install Git for Windows and use bash integration.

# Only initialize if wt is available
if (Get-Command {{ cmd }} -ErrorAction SilentlyContinue) {

    # wt wrapper function - captures stdout and executes as PowerShell
    function {{ cmd }} {
        param(
            [Parameter(ValueFromRemainingArguments = $true)]
            [string[]]$Arguments
        )

        $wtBin = (Get-Command {{ cmd }} -CommandType Application).Source

        # Run wt with --internal=powershell
        # stdout is captured for Invoke-Expression (contains Set-Location directives)
        # stderr passes through to console in real-time (user messages, progress, errors)
        # Note: We do NOT use 2>&1 as that would merge stderr into the script variable
        $script = & $wtBin --internal=powershell @Arguments | Out-String
        $exitCode = $LASTEXITCODE

        # Execute the directive script (e.g., Set-Location) if command succeeded
        if ($exitCode -eq 0 -and $script.Trim()) {
            Invoke-Expression $script
        }

        # Propagate exit code so $? and $LASTEXITCODE are consistent for scripts/CI
        $global:LASTEXITCODE = $exitCode
        if ($exitCode -ne 0) {
            # Write error to set $? = $false without throwing
            Write-Error "wt exited with code $exitCode" -ErrorAction SilentlyContinue
        }
        return $exitCode
    }

    # Tab completion - generate clap's completer script and eval it
    # This registers Register-ArgumentCompleter with proper handling
    $env:COMPLETE = "powershell"
    try {
        & (Get-Command {{ cmd }} -CommandType Application) | Out-String | Invoke-Expression
    }
    finally {
        Remove-Item Env:\COMPLETE -ErrorAction SilentlyContinue
    }
}
