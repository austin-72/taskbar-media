@echo off
rem Publishes this folder to https://github.com/austin-72/taskbar-media
rem The commit message is the contents of commit-message.txt (written by Claude).
rem Commits are made with the GitHub privacy (no-reply) email, set for this folder only.
rem What it printed is saved to publish-github.log.
setlocal
cd /d "%~dp0"
set "REPO=https://github.com/austin-72/taskbar-media.git"
set "GIT_NAME=Austin"
set "GIT_EMAIL=11325054+austin-72@users.noreply.github.com"
set "LOG=%~dp0publish-github.log"

call :publish > "%LOG%" 2>&1
set "RESULT=%ERRORLEVEL%"
type "%LOG%"
echo.
if "%RESULT%"=="0" goto :ok
echo PUBLISH FAILED. The details are above and in publish-github.log.
pause
exit /b %RESULT%

:ok
echo Published to %REPO%
timeout /t 5 >nul
exit /b 0

:publish
where git >nul 2>&1
if errorlevel 1 goto :nogit
if not exist "commit-message.txt" goto :nomessage

rem First run: this folder is not a git repository yet. Connect it to GitHub
rem and line it up with what is already there, without touching any files here.
if exist ".git" goto :ready
git init -b main
if errorlevel 1 exit /b 1
git remote add origin "%REPO%"
git fetch origin main
if errorlevel 1 exit /b 1
git reset --mixed origin/main
if errorlevel 1 exit /b 1

:ready
git remote set-url origin "%REPO%"
git config user.name "%GIT_NAME%"
git config user.email "%GIT_EMAIL%"

git add -A
if errorlevel 1 exit /b 1
git diff --cached --quiet
if not errorlevel 1 goto :push
git commit -F "commit-message.txt"
if errorlevel 1 exit /b 1

:push
git push -u origin main
if errorlevel 1 exit /b 1
exit /b 0

:nogit
echo Git was not found. Install Git for Windows from https://git-scm.com and run this again.
exit /b 1

:nomessage
echo commit-message.txt is missing, so there is no commit message to use.
exit /b 1
