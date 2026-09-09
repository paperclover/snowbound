if A_ScreenWidth != 800 || A_ScreenHeight != 600
    throw Error("This fixture requires an 800 by 600 desktop")
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{5A2F2B1E-5560-41EF-8323-49E6567546A9}{1}{B0}", "", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{5A2F2B1E-5560-41EF-8323-49E6567546A9}{1}{B0}", "", false)
Sleep 300
