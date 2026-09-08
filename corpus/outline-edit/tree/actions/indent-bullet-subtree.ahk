OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{F42F95BB-4229-49D9-9535-6B3742F22B14}{1}{B0}", "{CD04E749-A732-41FB-8966-A7D752A34819}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{F42F95BB-4229-49D9-9535-6B3742F22B14}{1}{B0}", "{CD04E749-A732-41FB-8966-A7D752A34819}{46}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Right}"
Sleep 300
