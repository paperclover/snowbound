#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{BE6416CA-DCE5-4E1E-8258-1F4E46F2EE1F}{1}{B0}", "{54CC9973-5534-4978-B65D-E22FE3045B84}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
app.NavigateTo("{BE6416CA-DCE5-4E1E-8258-1F4E46F2EE1F}{1}{B0}", "{54CC9973-5534-4978-B65D-E22FE3045B84}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
