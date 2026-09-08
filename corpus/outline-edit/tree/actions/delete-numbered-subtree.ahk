OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{9E821A55-182D-4F8E-B4E5-D1AF397CE319}{1}{B0}", "{A02053C2-3268-46C4-9BE5-0DF68966C6FD}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{9E821A55-182D-4F8E-B4E5-D1AF397CE319}{1}{B0}", "{A02053C2-3268-46C4-9BE5-0DF68966C6FD}{46}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
