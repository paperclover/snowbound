#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{6098C8BD-5256-42BE-A5A7-561BF30DE077}{1}{B0}", "{0355EDE0-60F1-4C03-BF26-8EB483A0F5AA}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
Sleep 300
Send "{Left}X"
Sleep 300
