@echo off
cd /d "%~dp0"
if not defined WIN7_TOKEN if exist "token.txt" set /p WIN7_TOKEN=<token.txt
start "win7-agent" /min "%~dp0vendor\python\python.exe" "%~dp0agent.py"
