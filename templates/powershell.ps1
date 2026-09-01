# worktrunk shell integration for PowerShell
#
# Limitations compared to bash/zsh/fish:
# - Hooks using bash syntax won't work without Git Bash
#
# For full hook compatibility on Windows, install Git for Windows and use bash integration.

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if ((Get-Command {{ cmd }} -ErrorAction SilentlyContinue) -or $env:WORKTRUNK_BIN) {

    # wt wrapper function - uses a temp file for the parent-shell cd directive
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

        try {
            # Run wt with the parent-shell cd directive
            $env:WORKTRUNK_DIRECTIVE_CD_FILE = $cdFile
            & $wtBin @args
            $exitCode = $LASTEXITCODE
        }
        finally {
            Remove-Item Env:\WORKTRUNK_DIRECTIVE_CD_FILE -ErrorAction SilentlyContinue
        }

        # Process the directive and clean it up even if cd throws.
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

        }
        finally {
            Remove-Item $cdFile -ErrorAction SilentlyContinue
        }

        # Propagate the exit code so scripts/CI can read $LASTEXITCODE.
        # Do NOT `return $exitCode`: in a PowerShell function `return <value>`
        # writes the value to the output (success) stream, so it would append a
        # stray exit-code line (e.g. `0`) after the real command output — that
        # corrupts captures like `$out = wt list --format json`. Setting
        # $global:LASTEXITCODE is what propagates the failure; the Write-Error
        # below surfaces a visible error record. Note: because this is a simple
        # function, Write-Error does NOT set the caller's $?, so don't rely on
        # $? reflecting a wt failure.
        $global:LASTEXITCODE = $exitCode
        if ($exitCode -ne 0) {
            # Surface a visible error record without throwing (does not set the
            # caller's $? from a simple function; $LASTEXITCODE carries the code)
            Write-Error "wt exited with code $exitCode" -ErrorAction SilentlyContinue
        }
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
    # Register under the name this integration binds. clap names its
    # Register-ArgumentCompleter target after its own command name, so without
    # this the --cmd name is never registered and nothing completes (#3816).
    $env:WORKTRUNK_COMPLETE_NAME = "{{ cmd }}"
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
        Remove-Item Env:\WORKTRUNK_COMPLETE_NAME -ErrorAction SilentlyContinue
    }
}
