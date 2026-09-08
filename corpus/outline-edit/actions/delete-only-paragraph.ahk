#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{2C90CD3D-EE7B-4FD1-9FE6-F67DE4BD37C2}{1}{B0}", "{40332BAB-F688-42D0-879E-15011BE0321C}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{2C90CD3D-EE7B-4FD1-9FE6-F67DE4BD37C2}{1}{B0}", "{40332BAB-F688-42D0-879E-15011BE0321C}{40}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
