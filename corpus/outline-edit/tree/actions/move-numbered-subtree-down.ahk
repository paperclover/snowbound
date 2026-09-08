OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{1FD29A18-8CDC-4762-997D-7007C95E11C9}{1}{B0}", "{75E97E0B-8D9D-4301-BC12-B1CB7F821418}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{1FD29A18-8CDC-4762-997D-7007C95E11C9}{1}{B0}", "{75E97E0B-8D9D-4301-BC12-B1CB7F821418}{46}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Down}"
Sleep 300
