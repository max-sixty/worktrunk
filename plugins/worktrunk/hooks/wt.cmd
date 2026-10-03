@echo off
rem Windows counterpart of the `bash "$PLUGIN_ROOT/hooks/wt.sh"` that the Unix
rem hook commands lead with: it finds Git Bash by path, then runs wt.sh through
rem it, so every integration resolves the worktrunk binary through that one
rem script.
rem
rem Codex runs a hook command through a shell, where a bare `bash` resolves
rem through PATH to System32\bash.exe -- the WSL launcher, not Git Bash -- and in
rem a sandboxed session refuses to start at all (#4007). Only the bare name is
rem the problem, so this resolves bash.exe the way find_git_bash does in
rem src/shell_exec.rs and changes nothing else.
rem Usage: wt.cmd [args...]
rem
rem This shim always exits 0, because it is where the hooks' `|| true` lives. The
rem shell running `commandWindows` is the session shell -- PowerShell on most
rem Windows machines, cmd.exe only when Codex knows of no session shell (#4239) --
rem and no spelling of `|| true` parses in both, so the swallow cannot live in
rem the hook command line. A marker is decoration; a nonzero exit is what raises
rem Codex's repeated "Hook failed" banner. Diagnostics still go to stderr.
rem
rem Every branch here is a bare `goto` or `call`: `if <cond> <cmd1> & <cmd2>`
rem runs cmd2 unconditionally, and `if <cond> <cmd1> || <cmd2>` tests the `if`
rem rather than cmd1, so neither connector can carry the control flow.
setlocal EnableExtensions
rem Clear the local: `setlocal` inherits the caller's environment, and Codex
rem hands each hook the session env snapshot, so an inherited BASH would name
rem what every event runs.
set "BASH="

rem git.exe installs at Git\cmd\git.exe or Git\bin\git.exe, and bash.exe at
rem Git\bin\bash.exe or Git\usr\bin\bash.exe. The lookup uses `where`'s `$var:`
rem prefix, which searches the directories named in that variable and nowhere
rem else: a bare `where git.exe` searches the current directory first, as cmd's
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

rem %1 is a Git install's cmd\ or bin\ directory, with the trailing separator
rem `%~dpI` leaves on. Git\bin\bash.exe first, as find_git_bash does: it is the
rem wrapper that sets up the MSYS environment for a caller outside Git Bash,
rem which is what puts `uname` and friends within reach of wt.sh.
:derive
call :accept "%~1..\bin\bash.exe"
if not defined BASH call :accept "%~1..\usr\bin\bash.exe"
goto :eof

:accept
if exist "%~1" set "BASH=%~f1"
goto :eof

:missing
echo worktrunk: Git for Windows is required but bash.exe was not found. Install from https://git-scm.com/download/win 1>&2
exit /b 0
