OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{CF318236-2DF3-48B2-8865-2AD5B5623F59}{1}{B0}", "{D02D46C5-FC21-45B6-BE8C-53D8012F37AD}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{CF318236-2DF3-48B2-8865-2AD5B5623F59}{1}{B0}", "{D02D46C5-FC21-45B6-BE8C-53D8012F37AD}{40}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
