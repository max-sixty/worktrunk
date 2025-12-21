# worktrunk shell integration for PowerShell
#
# Limitations compared to bash/zsh/fish:
# - Hooks using bash syntax won't work without Git Bash
#
# For full hook compatibility on Windows, install Git for Windows and use bash integration.

# Only initialize if wt is available
if (Get-Command {{ cmd }} -ErrorAction SilentlyContinue) {

    # wt wrapper function - uses temp file for directives
    function {{ cmd }} {
        param(
            [Parameter(ValueFromRemainingArguments = $true)]
            [string[]]$Arguments
        )

        $wtBin = (Get-Command {{ cmd }} -CommandType Application).Source
        $directiveFile = [System.IO.Path]::GetTempFileName()

        try {
            # Run wt with WORKTRUNK_DIRECTIVE_FILE env var
            # WORKTRUNK_SHELL tells the binary to use PowerShell-compatible escaping
            # stdout and stderr both go to console normally
            $env:WORKTRUNK_DIRECTIVE_FILE = $directiveFile
            $env:WORKTRUNK_SHELL = "powershell"
            & $wtBin @Arguments
            $exitCode = $LASTEXITCODE
        }
        finally {
            Remove-Item Env:\WORKTRUNK_DIRECTIVE_FILE -ErrorAction SilentlyContinue
            Remove-Item Env:\WORKTRUNK_SHELL -ErrorAction SilentlyContinue
        }

        # Execute the directive script if it has content
        try {
            if ((Test-Path $directiveFile) -and (Get-Item $directiveFile).Length -gt 0) {
                $script = Get-Content -Path $directiveFile -Raw
                if ($script.Trim()) {
                    Invoke-Expression $script
                    # If wt succeeded, use the directive script's exit code
                    if ($exitCode -eq 0) {
                        $exitCode = $LASTEXITCODE
                    }
                }
            }
        }
        finally {
            # Cleanup even if Invoke-Expression throws
            Remove-Item $directiveFile -ErrorAction SilentlyContinue
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
