@echo off
rem Windows counterpart of wt.sh: resolves the worktrunk CLI for plugin hooks.
rem
rem Codex runs hook commands through `cmd.exe /C` on Windows, where a bare
rem `bash` resolves through PATH to System32\bash.exe -- the WSL launcher, not
rem Git Bash -- and refuses to start in a sandboxed session (#4007). The Codex
rem hooks therefore reach worktrunk through this shim rather than wt.sh.
rem
rem If WORKTRUNK_BIN is set, uses that path exclusively. Otherwise prefers
rem git-wt.exe over wt, and rejects wt when PATH resolves it to Windows
rem Terminal (...\WindowsApps\wt.exe), which owns that name on Windows.
rem Usage: wt.cmd [args...]
rem
rem Every branch here is a bare `goto`: `if <cond> <cmd1> & <cmd2>` runs cmd2
rem unconditionally, and `if <cond> <cmd1> || <cmd2>` tests the `if` rather than
rem cmd1, so neither connector can carry the control flow.
setlocal EnableExtensions
rem Clear the locals: `setlocal` inherits the caller's environment, and an
rem inherited WT would short-circuit the PATH scan below onto whatever it names.
set "WT="
set "WT_SEEN="

if defined WORKTRUNK_BIN goto :override

rem Both PATH lookups use `where`'s `$var:` prefix, which searches the
rem directories named in that variable and nowhere else, and :run executes the
rem absolute path they return. A bare `where git-wt.exe` searches the current
rem directory first, as cmd's own bare-name lookup does, and a hook's current
rem directory is the user's project -- so a `git-wt.exe` or `wt.exe` committed
rem to a repo would otherwise be what every event executes. wt.sh has no such
rem surface: `command -v` consults PATH only.
for /f "delims=" %%I in ('where "$PATH:git-wt.exe" 2^>nul') do if not defined WT set "WT=%%I"
if defined WT goto :run

rem A cargo install builds no git-wt (that binary is behind a non-default
rem feature), so fall back to whichever `wt` PATH resolves -- unless that is
rem Windows Terminal, which would open a window instead of setting a marker.
for /f "delims=" %%I in ('where "$PATH:wt.exe" 2^>nul') do (
    set "WT_SEEN=1"
    if not defined WT call :accept "%%I"
)
if defined WT goto :run

if defined WT_SEEN echo worktrunk: 'wt' resolves to Windows Terminal; install worktrunk as git-wt.exe or remove the Windows Terminal alias. See https://worktrunk.dev/worktrunk/#install 1>&2
if not defined WT_SEEN echo worktrunk: could not find 'wt' in PATH 1>&2
exit /b 1

rem Take this PATH entry unless it is Windows Terminal's app-execution alias.
:accept
echo %1 | findstr /i /c:"WindowsApps" >nul
if errorlevel 1 set "WT=%~1"
goto :eof

:override
set "WT=%WORKTRUNK_BIN%"
goto :run

:run
"%WT%" %*
exit /b %ERRORLEVEL%
