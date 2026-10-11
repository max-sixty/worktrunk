@echo off
rem Windows hook entrypoint: find Git Bash by installation path, avoiding the
rem System32\bash.exe WSL launcher (#4007), then run the shared wt.sh.
rem Usage: wt.cmd [args...]
rem
rem Codex uses the session shell, so commandWindows must parse in cmd and both
rem PowerShell editions (#4239). `cmd /d /c call "${PLUGIN_ROOT}\hooks\wt.cmd"`
rem starts with a command rather than a PowerShell string expression. `call`
rem avoids cmd's initial-quote stripping; /d disables AutoRun for this cmd hop.
rem The outer cmd, if Codex uses one, still runs its own AutoRun. A cmd-session
rem lookup of the bare `cmd` can also resolve an executable in the project cwd.
rem
rem Always exit 0: markers are best-effort, and cmd's `|| exit /b 0` cannot live
rem in the cross-shell command string. Diagnostics still go to stderr.
rem
rem Every branch here is a bare `goto` or `call`: `if <cond> <cmd1> & <cmd2>`
rem runs cmd2 unconditionally, and `if <cond> <cmd1> || <cmd2>` tests the `if`
rem rather than cmd1, so neither connector can carry the control flow.
setlocal EnableExtensions
rem Clear the local: `setlocal` inherits the caller's environment, and Codex
rem hands each hook the session env snapshot, so an inherited BASH would name
rem what every event runs.
set "BASH="

rem git.exe installs at Git\cmd\git.exe, Git\bin\git.exe, or
rem Git\mingw64\bin\git.exe, and bash.exe at Git\bin\bash.exe or
rem Git\usr\bin\bash.exe. The lookup uses `where`'s `$var:` prefix, which
rem searches the directories named in that variable and nowhere else: a bare
rem `where git.exe` searches the current directory first, as cmd's
rem own bare-name lookup does, and a hook's current directory is the user's
rem project -- so a `git.exe` committed to a repo would otherwise choose the
rem bash every event runs. `where` itself is spelled absolutely for the same
rem reason: cmd resolves that name from the current directory too.
for /f "delims=" %%I in ('"%SystemRoot%\System32\where.exe" "$PATH:git.exe" 2^>nul') do if not defined BASH call :derive "%%~dpI"

rem A git.exe PATH doesn't name, or names through a shim outside its install:
rem the system-wide default, then the per-user one an install without admin
rem rights writes (#1259).
if not defined BASH call :accept "%ProgramFiles%\Git\bin\bash.exe"
if not defined BASH call :accept "%LOCALAPPDATA%\Programs\Git\bin\bash.exe"

if not defined BASH goto :missing

"%BASH%" "%~dp0wt.sh" %*
exit /b 0

rem %1 is a Git install's cmd\, bin\, or mingw64\bin\ directory, with the
rem trailing separator `%~dpI` leaves on; the last sits a level deeper, so the
rem install root is tried one level further up when the first two miss.
rem Git\bin\bash.exe first, as find_git_bash does: it is the wrapper that
rem sets up the MSYS environment for a caller outside Git Bash, which is what
rem puts `uname` and friends within reach of wt.sh.
:derive
call :accept "%~1..\bin\bash.exe"
if not defined BASH call :accept "%~1..\usr\bin\bash.exe"
if not defined BASH call :accept "%~1..\..\bin\bash.exe"
if not defined BASH call :accept "%~1..\..\usr\bin\bash.exe"
goto :eof

:accept
if exist "%~1" set "BASH=%~f1"
goto :eof

:missing
echo worktrunk: Git for Windows is required but bash.exe was not found. Install from https://git-scm.com/download/win 1>&2
exit /b 0
