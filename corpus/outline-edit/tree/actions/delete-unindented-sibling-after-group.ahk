OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{8848F261-93E9-4A78-8FDB-9304636C2572}{1}{B0}", "{F91A5591-BDC7-4F4E-A177-6C923E93C02D}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{8848F261-93E9-4A78-8FDB-9304636C2572}{1}{B0}", "{F91A5591-BDC7-4F4E-A177-6C923E93C02D}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
