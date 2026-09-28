OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{9193CD4B-2F8C-055F-3BDB-4EFF291B5152}{1}{B0}", "", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{9193CD4B-2F8C-055F-3BDB-4EFF291B5152}{1}{B0}", "", false)
Sleep 300
Sleep(2000)
