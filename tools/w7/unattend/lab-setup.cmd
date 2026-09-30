@echo off
rem First logon of a Windows 10/11 lab build: install the agent and quiet the desktop.
set "AGENT="
for %%D in (D E F G H I J K L M N O P Q R S T U V W X Y Z) do if exist "%%D:\agent.py" set "AGENT=%%D:"
if not defined AGENT exit /b 1
xcopy /e /i /y /q "%AGENT%\" C:\win7-agent\ || exit /b 1
attrib -r /s /d "C:\win7-agent\*"
netsh advfirewall firewall add rule name="win7-agent" dir=in action=allow protocol=TCP localport=8777
net accounts /maxpwage:unlimited
set WINLOGON=HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon
reg add "%WINLOGON%" /v AutoAdminLogon /t REG_SZ /d 1 /f
reg add "%WINLOGON%" /v DefaultUserName /t REG_SZ /d one /f
reg add "%WINLOGON%" /v DefaultPassword /t REG_SZ /d one /f
reg delete "%WINLOGON%" /v AutoLogonCount /f
powercfg /change monitor-timeout-ac 0
powercfg /change standby-timeout-ac 0
powercfg /hibernate off
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows\Personalization" /v NoLockScreen /t REG_DWORD /d 1 /f
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU" /v NoAutoUpdate /t REG_DWORD /d 1 /f
reg add "HKCU\Control Panel\Desktop" /v ScreenSaveActive /t REG_SZ /d 0 /f
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize" /v EnableTransparency /t REG_DWORD /d 1 /f
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement" /v ScoobeSystemSettingEnabled /t REG_DWORD /d 0 /f
call C:\win7-agent\install-autostart.cmd
shutdown /r /t 5
