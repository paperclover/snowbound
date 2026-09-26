# Extended ASCII text

`TextExtendedAscii` (`0x1c003498`, MS-ONE 2.2.89) names no code page. Each `candidate`
is `m6/ansi/notebook` with the paragraph's `RichEditTextLangID` (`0x10001cfe`) set to
0x0409 (`en`), 0x0419 (`ru`, ANSI code page 1251) or 0x0411 (`ja`, code page 932); its
text holds the bytes 0x80–0xFF. `cold` is each one's fresh OneNote 2010 read: all three
export U+0080–U+00FF, so OneNote reads the bytes as Latin-1 whatever the language, and
`read/page-000.png` shows 0x80–0x9F as nothing and 0xC0 as À rather than Cyrillic А.
OneNote left the section files unchanged.
