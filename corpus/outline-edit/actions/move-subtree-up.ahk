#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{FD63FFC3-A09F-42E1-BF61-EBD58FCFBACA}{1}{B0}", "{B71CF005-68C1-4007-8B1C-4ADC9BCA8D40}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{FD63FFC3-A09F-42E1-BF61-EBD58FCFBACA}{1}{B0}", "{B71CF005-68C1-4007-8B1C-4ADC9BCA8D40}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Up}"
Sleep 300
