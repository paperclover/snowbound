param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -gt 0) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($pages.Count -ne 1) { throw 'The direction fixture requires one page.' }
    $content = ''
    $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 1, 1)
    [IO.File]::WriteAllText("$Root\before.xml", $content, [Text.Encoding]::UTF8)
    [xml]$page = $content
    $settings = $page.SelectSingleNode('//*[local-name()="PageSettings"]')
    $settings.SetAttribute('RTL', 'true')
    $columns = @($page.SelectNodes('//*[local-name()="Column"]'))
    for ($i = 0; $i -lt $columns.Count; $i++) {
        $columns[$i].SetAttribute('width', (96 + 48 * $i).ToString())
        $columns[$i].SetAttribute('isLocked', 'true')
    }
    $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $true)
    $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 1, 1)
    [IO.File]::WriteAllText("$Root\after.xml", $content, [Text.Encoding]::UTF8)
    [xml]$after = $content
    if ($after.SelectSingleNode('//*[local-name()="PageSettings"]').GetAttribute('RTL') -ne 'true') { throw 'Native page direction did not change.' }
    $app.SyncHierarchy($notebook)
    $app.CloseNotebook($notebook, $false)
    $notebook = ''
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
