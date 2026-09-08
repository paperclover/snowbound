#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{AD28524E-CC50-40D0-84FA-157F9867EC05}{1}{B0}", "{E971E60D-4048-4A2C-9151-0CE852F4405D}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
Sleep 300
Send "{Right}{Enter}{Tab}Native child"
Sleep 300
