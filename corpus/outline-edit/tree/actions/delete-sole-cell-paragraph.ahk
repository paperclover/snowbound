OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{A9AF5DEC-514F-45E5-B57F-5F7CDF446F72}{1}{B0}", "{42D17B21-AB2F-49CB-9115-14A2BF0D6727}{30}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{A9AF5DEC-514F-45E5-B57F-5F7CDF446F72}{1}{B0}", "{42D17B21-AB2F-49CB-9115-14A2BF0D6727}{30}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
