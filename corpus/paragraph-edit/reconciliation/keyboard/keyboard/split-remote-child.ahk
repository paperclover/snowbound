#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{AA94EAE9-FD63-4922-86A1-458933A73EA4}{1}{B0}", "{92829D34-66A4-48FE-AC75-907125308AE5}{40}{B0}", false)
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
