OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{D8F75D3B-62F9-41BC-8C92-2C3257BEB470}{1}{B0}", "{B3C20949-3D36-4334-A2B1-24DB43118FCF}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{D8F75D3B-62F9-41BC-8C92-2C3257BEB470}{1}{B0}", "{B3C20949-3D36-4334-A2B1-24DB43118FCF}{46}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Down}"
Sleep 300
