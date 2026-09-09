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
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -eq 9) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    $targets = @($pages | Where-Object { $_.GetAttribute('name') -eq 'Same title' })
    if ($pages.Count -ne 9 -or $targets.Count -ne 1) { throw 'Expected nine pages and one moved parent.' }
    $pageId = [Security.SecurityElement]::Escape($targets[0].GetAttribute('ID'))
    $title = 'Native moved ' + [char]::ConvertFromUtf32(0x1F98B) + ' e' + [char]0x301
    $title = [Security.SecurityElement]::Escape($title)
    $xml = "<one:Page xmlns:one='http://schemas.microsoft.com/office/onenote/2010/onenote' ID='$pageId'><one:Title><one:OE><one:T>$title</one:T></one:OE></one:Title><one:Outline><one:Position x='72' y='144'/><one:OEChildren><one:OE><one:T>Native body after a Rust page move.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>"
    [IO.File]::WriteAllText((Join-Path $Root 'native-edit.xml'), $xml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebook)
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
