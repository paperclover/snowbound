#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{3F7D85A7-2787-4DAA-B157-6E9FCFF94F28}{1}{B0}", "{8B6736A9-E8D2-47EA-BE99-3B0D9C0B147E}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{3F7D85A7-2787-4DAA-B157-6E9FCFF94F28}{1}{B0}", "{8B6736A9-E8D2-47EA-BE99-3B0D9C0B147E}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Down}"
Sleep 300
