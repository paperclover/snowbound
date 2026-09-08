#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{ED4F957C-5F6C-49AA-A0CD-A58F8B941BE8}{1}{B0}", "{D2D2C591-9007-4DF6-BD05-7F0BD6BDEFD9}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{ED4F957C-5F6C-49AA-A0CD-A58F8B941BE8}{1}{B0}", "{D2D2C591-9007-4DF6-BD05-7F0BD6BDEFD9}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Right}"
Sleep 300
