@echo off
setlocal EnableExtensions
set "ONEVM_HOSTNAME="
for %%D in (D E F G H I J K L M N O P Q R S T U V W X Y Z) do if exist "%%D:\onenote-vm.ini" for /f "usebackq tokens=1,* delims==" %%A in ("%%D:\onenote-vm.ini") do (
  if /i "%%A"=="HOSTNAME" set "ONEVM_HOSTNAME=%%B"
  if /i "%%A"=="TOKEN" set "WIN7_TOKEN=%%B"
)
if not defined ONEVM_HOSTNAME goto agent
if /i "%COMPUTERNAME%"=="%ONEVM_HOSTNAME%" goto agent
rem Windows 11 ships without wmic; Windows 7 PowerShell lacks Rename-Computer.
where wmic >nul 2>&1 || goto rename_powershell
wmic computersystem where name="%COMPUTERNAME%" call rename name="%ONEVM_HOSTNAME%" >"%~dp0bootstrap.log" 2>&1
find "ReturnValue = 0;" "%~dp0bootstrap.log" >nul || exit /b 1
goto restart
:rename_powershell
powershell -NoProfile -Command "Rename-Computer -NewName '%ONEVM_HOSTNAME%' -Force -ErrorAction Stop" >"%~dp0bootstrap.log" 2>&1 || exit /b 1
:restart
shutdown /r /t 0
exit /b

:agent
call "%~dp0run.bat"
