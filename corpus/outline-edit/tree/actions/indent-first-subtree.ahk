OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{B98DD413-992C-48FE-BAC1-2C734ED5CE69}{1}{B0}", "{76D86304-5E43-49D0-98EC-69FFF5011C8F}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{B98DD413-992C-48FE-BAC1-2C734ED5CE69}{1}{B0}", "{76D86304-5E43-49D0-98EC-69FFF5011C8F}{40}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Right}"
Sleep 300
