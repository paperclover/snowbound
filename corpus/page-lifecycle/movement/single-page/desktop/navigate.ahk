if A_ScreenWidth != 800 || A_ScreenHeight != 600
    throw Error("This fixture requires an 800 by 600 desktop")
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{C6DF1EEC-1EB0-4FE5-815D-5159ABCB4C73}{1}{B0}", "", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{C6DF1EEC-1EB0-4FE5-815D-5159ABCB4C73}{1}{B0}", "", false)
Sleep 300
