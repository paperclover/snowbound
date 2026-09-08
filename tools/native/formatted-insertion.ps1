param([Parameter(Mandatory=$true)][string]$Root, [string]$CloneHost = '')
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
        if ($pages.Count -eq 1) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($pages.Count -ne 1) { throw 'Expected one fixture page.' }
    $pageId = $pages[0].GetAttribute('ID')
    foreach ($phase in @('control', 'edit')) {
        $content = ''
        $app.GetPageContent($pageId, [ref]$content, 1, 1)
        [IO.File]::WriteAllText((Join-Path $Root ($phase + '-before.xml')), $content, [Text.Encoding]::UTF8)
        [xml]$page = $content
        $matches = @($page.SelectNodes('//*[local-name()="T"]') | Where-Object {
            $plain = [regex]::Replace($_.InnerText, '<[^>]*>', '')
            $plain.StartsWith('Bold ') -or $plain.StartsWith('Typed ')
        })
        if ($matches.Count -ne 1) { throw 'Expected one inserted rich paragraph.' }
        $target = $matches[0]
        if ($phase -eq 'edit') { $target.InnerText = $target.InnerText.Replace('Bold', 'Native Bold').Replace('Typed', 'Native Typed') }
        $outline = $target
        while ($outline.LocalName -ne 'Outline') { $outline = $outline.ParentNode }
        $update = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $pageId + '">'
        foreach ($style in $page.SelectNodes('/*/*[local-name()="QuickStyleDef"]')) { $update += $style.OuterXml }
        if ($outline.SelectSingleNode('.//*[local-name()="Table"]')) {
            # Resubmitting measured outline size constrains native table auto-fit.
            foreach ($size in @($outline.SelectNodes('*[local-name()="Size"]'))) { [void]$outline.RemoveChild($size) }
        }
        $update += $outline.OuterXml + '</one:Page>'
        [IO.File]::WriteAllText((Join-Path $Root ($phase + '-update.xml')), $update, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($update, [DateTime]::MinValue, 1, $false)
    }
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
