# worktrunk shell integration for PowerShell
#
# NOTE: PowerShell integration has limited functionality compared to bash/zsh/fish.
# For full functionality on Windows, use Git Bash with `wt config shell install bash`.
#
# Limitations:
# - No directory change support (PowerShell can't change parent process directory)
# - Hooks that use bash syntax won't work without Git Bash
#
# This integration provides basic command execution and completions.

# Only initialize if wt is available
if (Get-Command {{ cmd_prefix }} -ErrorAction SilentlyContinue) {

    # wt wrapper function
    # Unlike POSIX shells, PowerShell can't eval shell scripts from stdout
    # So we just run wt directly - no --internal mode support
    function {{ cmd_prefix }} {
        param(
            [Parameter(ValueFromRemainingArguments = $true)]
            [string[]]$Arguments
        )

        # Filter out --source flag (not supported in PowerShell wrapper)
        $filteredArgs = $Arguments | Where-Object { $_ -ne "--source" }

        # Run wt directly (no directive mode - PowerShell can't eval shell scripts)
        & (Get-Command {{ cmd_prefix }} -CommandType Application) @filteredArgs
    }

    # Tab completion using clap's PowerShell completer
    Register-ArgumentCompleter -Native -CommandName {{ cmd_prefix }} -ScriptBlock {
        param($wordToComplete, $commandAst, $cursorPosition)

        $env:COMPLETE = "powershell"
        try {
            # Get the command line up to the cursor
            $commandLine = $commandAst.ToString()

            # Call wt with completion environment
            & (Get-Command {{ cmd_prefix }} -CommandType Application) $commandLine.Split() 2>$null | ForEach-Object {
                [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
            }
        }
        finally {
            Remove-Item Env:\COMPLETE -ErrorAction SilentlyContinue
        }
    }
}
