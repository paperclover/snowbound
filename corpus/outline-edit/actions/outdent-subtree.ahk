#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{0BDFFEA3-F8E9-4A45-B9E5-D249715CDC0B}{1}{B0}", "{92D67C94-716A-4489-AB52-B671E385FFA2}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{0BDFFEA3-F8E9-4A45-B9E5-D249715CDC0B}{1}{B0}", "{92D67C94-716A-4489-AB52-B671E385FFA2}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Left}"
Sleep 300
