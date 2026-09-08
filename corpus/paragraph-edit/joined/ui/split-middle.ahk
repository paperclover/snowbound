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
app.NavigateTo("{D2B90D6D-D72E-4720-9D2D-17121D2F2774}{1}{B0}", "{27380AFB-64A9-4D62-B4A6-00F074A62D13}{11}{B0}", false)
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
if !hwnd
    throw Error("OneNote window did not appear")
WinMaximize(hwnd)
WinActivate(hwnd)
if !WinWaitActive(hwnd,, 10)
    throw Error("OneNote window did not become active")
Sleep 300
Send "{Left}{Backspace}"
