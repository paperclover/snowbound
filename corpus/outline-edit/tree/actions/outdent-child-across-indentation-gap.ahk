OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{0021D1AD-E6E0-4DFE-8292-B0D7F560ECC3}{1}{B0}", "{2511F1B2-87DC-4B09-BFBF-2AA175972EFD}{45}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{0021D1AD-E6E0-4DFE-8292-B0D7F560ECC3}{1}{B0}", "{2511F1B2-87DC-4B09-BFBF-2AA175972EFD}{45}{B0}", false)
Sleep 300
Send "{Left}^+{-}!+{Left}"
Sleep 300
