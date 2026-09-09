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
        if ($pages.Count -eq 13) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    $targets = @($pages | Where-Object { $_.GetAttribute('name').StartsWith('New ') })
    if ($pages.Count -ne 13 -or $targets.Count -ne 2) { throw 'Expected thirteen pages and two created title controls.' }
    $inputs = Join-Path $Root 'inputs'
    [void](New-Item -ItemType Directory -Path $inputs)
    for ($i = 0; $i -lt $targets.Count; $i++) {
        $pageId = [Security.SecurityElement]::Escape($targets[$i].GetAttribute('ID'))
        $xml = "<one:Page xmlns:one='http://schemas.microsoft.com/office/onenote/2010/onenote' ID='$pageId'><one:Outline><one:Position x='72' y='144'/><one:OEChildren><one:OE><one:T>Native body $i after Rust page creation.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>"
        [IO.File]::WriteAllText((Join-Path $inputs ("native-$i.xml")), $xml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    }
    $app.SyncHierarchy($notebook)
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
