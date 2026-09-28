OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{B491B045-3B02-0DB9-17A3-E19B54F1CDC1}{1}{B0}", "", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{B491B045-3B02-0DB9-17A3-E19B54F1CDC1}{1}{B0}", "", false)
Sleep 300
Sleep(2000)
