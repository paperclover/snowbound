#Requires AutoHotkey v2.0
OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))
dm := Buffer(220, 0)
NumPut("UShort", 220, dm, 68)
if !DllCall("EnumDisplaySettingsW", "Ptr", 0, "UInt", 0xFFFFFFFF, "Ptr", dm)
    throw Error("Cannot inspect display mode")
NumPut("UInt", NumGet(dm, 72, "UInt") | 0x180000, dm, 72)
NumPut("UInt", 1280, dm, 172)
NumPut("UInt", 720, dm, 176)
if DllCall("ChangeDisplaySettingsW", "Ptr", dm, "UInt", 0, "Int") != 0
    throw Error("Display mode rejected")
app := ComObject("OneNote.Application")
app.NavigateTo("{6F696EA1-B583-47E1-90CD-7D524EFA979B}{1}{B0}", "{2B5E855D-DE3A-4FBA-A000-B81E9D3B78F9}{40}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
Sleep 300
Send "{Left}{Right 2}{Enter}"
