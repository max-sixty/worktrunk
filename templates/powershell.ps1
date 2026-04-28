# worktrunk shell integration for PowerShell
#
# Limitations compared to bash/zsh/fish:
# - Hooks using bash syntax won't work without Git Bash
#
# For full hook compatibility on Windows, install Git for Windows and use bash integration.

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if ((Get-Command {{ cmd }} -ErrorAction SilentlyContinue) -or $env:WORKTRUNK_BIN) {

    # wt wrapper function - uses split temp files for directives
    #
    # IMPORTANT: This function must remain a "simple function" (no [CmdletBinding()] or
    # [Parameter()] attributes). Advanced functions add common parameters like -Debug,
    # -Verbose, -ErrorAction, etc. that intercept short flags: e.g., -D is consumed as
    # -Debug, -V as -Verbose, instead of being passed through to wt.exe. Using $args
    # (automatic variable for simple functions) ensures all arguments reach the binary
    # unchanged.
    function {{ cmd }} {
        # Use WORKTRUNK_BIN if set (for testing dev builds), otherwise find via Get-Command
        # Select-Object -First 1 handles case where multiple binaries match (e.g., wt.exe from Windows Terminal)
        if ($env:WORKTRUNK_BIN) {
            $wtBin = $env:WORKTRUNK_BIN
        } else {
            $wtBin = (Get-Command {{ cmd }} -CommandType Application | Select-Object -First 1).Source
        }

        $cdFile = [System.IO.Path]::GetTempFileName()
        $execFile = [System.IO.Path]::GetTempFileName()

        try {
            # Run wt with split directive env vars
            # WORKTRUNK_SHELL tells the binary to use PowerShell-compatible escaping (legacy compat)
            $env:WORKTRUNK_DIRECTIVE_CD_FILE = $cdFile
            $env:WORKTRUNK_DIRECTIVE_EXEC_FILE = $execFile
            $env:WORKTRUNK_SHELL = "powershell"
            & $wtBin @args
            $exitCode = $LASTEXITCODE
        }
        finally {
            Remove-Item Env:\WORKTRUNK_DIRECTIVE_CD_FILE -ErrorAction SilentlyContinue
            Remove-Item Env:\WORKTRUNK_DIRECTIVE_EXEC_FILE -ErrorAction SilentlyContinue
            Remove-Item Env:\WORKTRUNK_SHELL -ErrorAction SilentlyContinue
        }

        # Process directive files and clean up in a single try/finally so both
        # temp files are removed even if cd or exec throws.
        try {
            # cd file holds a raw path (no shell escaping needed)
            if ((Test-Path $cdFile) -and (Get-Item $cdFile).Length -gt 0) {
                $target = (Get-Content -Path $cdFile -Raw).Trim()
                if ($target) {
                    Set-Location -LiteralPath $target
                    if ($exitCode -eq 0) {
                        $exitCode = $LASTEXITCODE
                    }
                }
            }

            # exec file holds arbitrary shell (e.g. from --execute)
            if ((Test-Path $execFile) -and (Get-Item $execFile).Length -gt 0) {
                $script = Get-Content -Path $execFile -Raw
                if ($script.Trim()) {
                    Invoke-Expression $script
                    if ($exitCode -eq 0) {
                        $exitCode = $LASTEXITCODE
                    }
                }
            }
        }
        finally {
            Remove-Item $cdFile -ErrorAction SilentlyContinue
            Remove-Item $execFile -ErrorAction SilentlyContinue
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
    # Use WORKTRUNK_BIN if set (for testing), otherwise find via Get-Command
    if ($env:WORKTRUNK_BIN) {
        $wtBinForComplete = $env:WORKTRUNK_BIN
    } else {
        $wtBinForComplete = (Get-Command {{ cmd }} -CommandType Application | Select-Object -First 1).Source
    }
    $env:COMPLETE = "powershell"
    try {
        # Capture output first, then pipe - avoids "Cannot run a document in the middle of a pipeline"
        # error that can occur in some PowerShell configurations/terminals
        $completionScript = & $wtBinForComplete 2>$null
        if ($completionScript) {
            $completionScript | Out-String | Invoke-Expression
        }
    }
    catch {
        # Completion registration is optional - wrapper function still works without it
    }
    finally {
        Remove-Item Env:\COMPLETE -ErrorAction SilentlyContinue
    }
}
