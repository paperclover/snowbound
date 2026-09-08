#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{13DB2155-80B0-4789-8875-37E369FEB7C7}{1}{B0}", "{6FC1F2DD-27CB-41FF-88F4-6F22A1A31468}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{13DB2155-80B0-4789-8875-37E369FEB7C7}{1}{B0}", "{6FC1F2DD-27CB-41FF-88F4-6F22A1A31468}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Down}"
Sleep 300
