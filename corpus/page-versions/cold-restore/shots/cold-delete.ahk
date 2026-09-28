WinActivate("ahk_exe ONENOTE.EXE")
CoordMode("Mouse", "Screen")
Click(393, 128)
Sleep(2500)
if WinExist("ahk_class #32770 ahk_exe ONENOTE.EXE") {
    FileAppend("dialog: " WinGetTitle() " | " WinGetText() "`n", "*", "UTF-8")
}
