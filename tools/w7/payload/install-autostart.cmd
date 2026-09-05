@echo off
reg add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System" /v EnableLUA /t REG_DWORD /d 0 /f || exit /b 1
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v OneNoteWin7Agent /t REG_SZ /d "cmd.exe /c C:\win7-agent\bootstrap.cmd" /f || exit /b 1
echo Restart Windows to finish.
