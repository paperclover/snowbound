#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
hwnd := WinExist("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE")
if !hwnd
    throw Error("OneNote window is missing")
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 5)
    throw Error("OneNote window is not active")
Send "{Left}"
SendText "UI "
