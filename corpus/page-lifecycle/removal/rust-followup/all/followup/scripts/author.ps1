param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('Lifecycle.one', $notebook, [ref]$section, 0)
    $page = ''
    $app.CreateNewPage($section, [ref]$page, 2)
    $title = 'Native after Rust removal ' + [char]::ConvertFromUtf32(0x1F98B) + ' e' + [char]0x0301
    $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:Title><one:OE><one:T><![CDATA[' + $title + ']]></one:T></one:OE></one:Title><one:Outline><one:OEChildren><one:OE><one:T><![CDATA[<b>Native body after Rust removal.</b>]]></one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebook)
    [IO.File]::WriteAllText("$Root\native-title.txt", $title, [Text.Encoding]::UTF8)
} finally {
    try { if ($notebook) { $app.CloseNotebook($notebook, $false) } }
    finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 120 -ErrorAction SilentlyContinue
    }
}
