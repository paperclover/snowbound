#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{4E20F67A-9223-4BB5-9191-7EC10CE43F1F}{1}{B0}", "{3224CEC3-F663-4F74-8CBE-540DDC982B1E}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
Sleep 300
Send "{Left}{Right 2}X"
Sleep 300
