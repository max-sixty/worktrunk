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

rem Ask Git for its runtime path so PATH shims resolve to the installation
rem they launch. Ignore GIT_EXEC_PATH in the query: it overrides the installed
rem helper path. The hook itself keeps the caller's environment.
rem cmd's PATH modifier finds the first git.exe on PATH without consulting
rem the project directory or spawning another command interpreter for lookup.
for %%I in (git.exe) do if not "%%~$PATH:I"=="" call :derive "%%~$PATH:I"

if not defined BASH goto :missing

"%BASH%" "%~dp0wt.sh" %*
exit /b 0

rem Git for Windows reports <install>/<runtime>/libexec/git-core. The bin
rem wrapper sets up MSYS for wt.sh; usr/bin is the runtime Bash itself.
:derive
for /f "delims=" %%I in ('set "GIT_EXEC_PATH=" ^& "%~1" --exec-path 2^>nul') do call :runtime "%%I"
goto :eof

:runtime
rem Normalize Git's forward slashes before matching its runtime suffix.
set "GIT_RUNTIME=%~1"
set "GIT_RUNTIME=%GIT_RUNTIME:/=\%"
if /i not "%GIT_RUNTIME:~-17%"=="\libexec\git-core" goto :eof
call :accept "%GIT_RUNTIME%\..\..\..\bin\bash.exe"
if not defined BASH call :accept "%GIT_RUNTIME%\..\..\..\usr\bin\bash.exe"
goto :eof

:accept
if exist "%~1" set "BASH=%~f1"
goto :eof

:missing
echo worktrunk: Git for Windows is required but bash.exe was not found. Install from https://git-scm.com/download/win 1>&2
exit /b 0
