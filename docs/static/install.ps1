# Worktrunk Installer (Windows)
# https://worktrunk.dev/install.ps1

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

if ((Get-Command wt -ErrorAction SilentlyContinue) -and (wt --version 2>&1 | Select-String 'worktrunk')) {
    wt config shell install
} elseif (Get-Command git-wt -ErrorAction SilentlyContinue) {
    git-wt config shell install
} else {
    Write-Host ""
    Write-Host "Warning: worktrunk installed but neither 'wt' nor 'git-wt' found in PATH." -ForegroundColor Yellow
    Write-Host "Restart your shell and run 'wt config shell install' (or 'git-wt config shell install') manually."
}
