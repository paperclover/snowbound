OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
DetectHiddenWindows false
app := ComObject("OneNote.Application")
app.NavigateTo("{2E5F7E51-CCD6-45A6-8CF6-D073792509DB}{1}{B0}", "{B06FB066-49DD-4B3D-828B-E66AA11D889E}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
app.NavigateTo("{2E5F7E51-CCD6-45A6-8CF6-D073792509DB}{1}{B0}", "{B06FB066-49DD-4B3D-828B-E66AA11D889E}{46}{B0}", false)
Sleep 300
Send "{Left}^+{-}{Delete}"
Sleep 300
