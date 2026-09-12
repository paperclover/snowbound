param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('pictures.one', $notebookId, [ref]$sectionId, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $page = $null
    do {
        $hierarchy = ''
        $app.GetHierarchy($sectionId, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -eq 1) {
            $content = ''
            $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 3, 1)
            [xml]$page = $content
            if (@($page.SelectNodes('//*[local-name()="Image"]')).Count -eq 1) { break }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if (@($page.SelectNodes('//*[local-name()="Image"]')).Count -ne 1) { throw 'Expected one picture on the page.' }
    [IO.File]::WriteAllText((Join-Path $Root 'before.xml'), $content, [Text.Encoding]::UTF8)
    $data = $page.SelectSingleNode('//*[local-name()="Image"]/*[local-name()="Data"]').InnerText
    $image = $page.CreateElement('one', 'Image', $namespace)
    $image.SetAttribute('format', 'png')
    $image.SetAttribute('alt', 'Page-level picture')
    $position = $page.CreateElement('one', 'Position', $namespace)
    $position.SetAttribute('x', '360')
    $position.SetAttribute('y', '240')
    [void]$image.AppendChild($position)
    $size = $page.CreateElement('one', 'Size', $namespace)
    $size.SetAttribute('width', '96')
    $size.SetAttribute('height', '72')
    $size.SetAttribute('isSetByUser', 'true')
    [void]$image.AppendChild($size)
    $payload = $page.CreateElement('one', 'Data', $namespace)
    $payload.InnerText = $data
    [void]$image.AppendChild($payload)
    [void]$page.DocumentElement.AppendChild($image)
    [IO.File]::WriteAllText((Join-Path $Root 'update.xml'), $page.OuterXml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
