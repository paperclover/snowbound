OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{F8757FD3-BDA0-4187-B841-80EDF46DFEE9}{1}{B0}", "{6302145E-3541-466F-8370-48B83B7E2E53}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{F8757FD3-BDA0-4187-B841-80EDF46DFEE9}{1}{B0}", "{6302145E-3541-466F-8370-48B83B7E2E53}{40}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Left}"
Sleep 300
