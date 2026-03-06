# Worktrunk Installer (Windows)
# https://worktrunk.dev/install.ps1

if ($IsWindows -eq $false -and $PSVersionTable.PSVersion.Major -ge 6) {
    Write-Host "Non-Windows environment detected. Please use the shell installer instead:" -ForegroundColor Yellow
    Write-Host "  curl -fsSL https://worktrunk.dev/install.sh | sh"
    exit 1
}

Write-Host "Installing worktrunk..."
irm https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-installer.ps1 | iex

# cargo-dist installs to ~/.cargo/bin by default.
# On Windows, winget or direct install might use git-wt to avoid conflict with Windows Terminal.
$env:Path += ";$HOME\.cargo\bin"
if ((Get-Command wt -ErrorAction SilentlyContinue) -and (wt --version 2>&1 | Select-String 'worktrunk')) {
    wt config shell install
} elseif (Get-Command git-wt -ErrorAction SilentlyContinue) {
    git-wt config shell install
} else {
    Write-Host ""
    Write-Host "Warning: worktrunk installed but neither 'git-wt' nor 'wt' found in PATH." -ForegroundColor Yellow
    Write-Host "Please restart your shell and run 'git-wt config shell install' manually."
}
