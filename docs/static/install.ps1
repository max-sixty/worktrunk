# Worktrunk Installer (Windows)
# https://worktrunk.dev/install.ps1

$ErrorActionPreference = 'Stop'

if ($IsWindows -eq $false -and $PSVersionTable.PSVersion.Major -ge 6) {
    Write-Host "Non-Windows environment detected. Please use the shell installer instead:" -ForegroundColor Yellow
    Write-Host "  curl -fsSL https://worktrunk.dev/install.sh | sh"
    exit 1
}

Write-Host "Installing worktrunk..."
irm https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-installer.ps1 | iex

# Update PATH to pick up the newly installed binary.
# Respect CARGO_HOME if set, otherwise use the default location.
$cargoBin = if ($env:CARGO_HOME) { "$env:CARGO_HOME\bin" } else { "$HOME\.cargo\bin" }
if ($env:Path -notlike "*$cargoBin*") {
    $env:Path += ";$cargoBin"
}

# Check whether `wt` on PATH is actually worktrunk (Windows Terminal uses
# the same alias). Wrap in try/catch — with ErrorActionPreference='Stop',
# a failing `wt --version` would throw instead of falling through.
$wtIsWorktrunk = $false
if (Get-Command wt -ErrorAction SilentlyContinue) {
    try {
        $wtIsWorktrunk = [bool](wt --version 2>&1 | Select-String 'worktrunk')
    } catch {
        $wtIsWorktrunk = $false
    }
}

if ($wtIsWorktrunk) {
    wt config shell install
} elseif (Get-Command git-wt -ErrorAction SilentlyContinue) {
    git-wt config shell install
} else {
    Write-Host ""
    Write-Host "Warning: worktrunk installed but neither 'wt' nor 'git-wt' found in PATH." -ForegroundColor Yellow
    Write-Host "Restart your shell and run 'wt config shell install' (or 'git-wt config shell install') manually."
}
