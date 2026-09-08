#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
app := ComObject("OneNote.Application")
app.NavigateTo("{DEA76FCD-5853-477C-AD4A-F165915338BB}{1}{B0}", "{62483E36-C372-4F75-90BF-DE6D1F7A5FC1}{40}{B0}", false)
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
app.NavigateTo("{DEA76FCD-5853-477C-AD4A-F165915338BB}{1}{B0}", "{62483E36-C372-4F75-90BF-DE6D1F7A5FC1}{46}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote did not become active")
Sleep 300
Send "{Right}Y"
Sleep 300
