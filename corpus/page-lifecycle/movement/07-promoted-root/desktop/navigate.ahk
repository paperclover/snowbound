if A_ScreenWidth != 800 || A_ScreenHeight != 600
    throw Error("This fixture requires an 800 by 600 desktop")
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{90F6D9E4-5056-4A2A-B83D-26F482FE1391}{1}{B0}", "", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{90F6D9E4-5056-4A2A-B83D-26F482FE1391}{1}{B0}", "", false)
Sleep 300
