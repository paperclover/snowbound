#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{70252131-654A-4CB2-9015-3264EF828208}{1}{B0}", "{24C6ABE1-25B8-4183-90D0-07A1BFD645EF}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
Sleep 300
Send "{Left}^."
Sleep 300
