#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{03E12E10-4A4C-4C60-9643-7D516E223CD0}{1}{B0}", "{FBC38B3A-5810-4BD3-9608-0577DDB35904}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{03E12E10-4A4C-4C60-9643-7D516E223CD0}{1}{B0}", "{FBC38B3A-5810-4BD3-9608-0577DDB35904}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
