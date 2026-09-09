param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('TitleControl.one', $notebook, [ref]$section, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -eq 3) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($pages.Count -ne 3) { throw 'Expected three outline title controls.' }
    $inputs = Join-Path $Root 'inputs'
    [void](New-Item -ItemType Directory -Path $inputs)
    foreach ($page in $pages) {
        $name = $page.GetAttribute('name')
        if ($name -notin @('Vertical first', 'Horizontal first', 'Explicit title')) { throw 'Unexpected control title.' }
        $content = ''
        $app.GetPageContent($page.GetAttribute('ID'), [ref]$content, 1, 1)
        [xml]$xml = $content
        $outlines = @($xml.SelectNodes('/*/*[local-name()="Outline"]') | Where-Object { $_.InnerText -match 'Second' })
        if ($outlines.Count -ne 1) { throw 'Expected one movable outline.' }
        $outline = $outlines[0]
        $position = $outline.SelectSingleNode('./*[local-name()="Position"]')
        $position.SetAttribute('x', '36')
        $position.SetAttribute('y', $(if ($name -eq 'Horizontal first') { '108' } else { '72' }))
        foreach ($child in @($xml.DocumentElement.ChildNodes)) {
            if ($child -ne $outline -and $child.LocalName -notin @('QuickStyleDef', 'TagDef')) {
                [void]$xml.DocumentElement.RemoveChild($child)
            }
        }
        [IO.File]::WriteAllText((Join-Path $inputs ($name + '.xml')), $xml.OuterXml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $false)
    }
    $app.SyncHierarchy($notebook)
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
