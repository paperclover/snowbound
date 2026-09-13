param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
$log = @()
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $hierarchy = ''
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'structure-00-opened.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $second = ''
    $app.OpenHierarchy('Second.one', $notebookId, [ref]$second, 3)
    $third = ''
    $app.OpenHierarchy('Third.one', $notebookId, [ref]$third, 3)
    $group = ''
    $app.OpenHierarchy('Group', $notebookId, [ref]$group, 2)
    $inner = ''
    $app.OpenHierarchy('Inner.one', $group, [ref]$inner, 3)
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'structure-01-created.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $manager = New-Object Xml.XmlNamespaceManager($tree.NameTable)
    $manager.AddNamespace('one', $namespace)
    $sections = @($tree.SelectNodes('//one:Section', $manager))
    foreach ($section in $sections) {
        switch ($section.GetAttribute('name')) {
            'Second' { $section.SetAttribute('name', 'Renamed'); $section.SetAttribute('color', '#FFD75E') }
            'Third' { $section.SetAttribute('color', '#B7C997') }
        }
    }
    [IO.File]::WriteAllText((Join-Path $Root 'structure-02-update.xml'), $tree.OuterXml, [Text.Encoding]::UTF8)
    $app.UpdateHierarchy($tree.OuterXml, 1)
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'structure-03-renamed.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
    Copy-Item -Recurse (Join-Path $Root 'notebook') (Join-Path $Root 'notebook-renamed')
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        if (@($tree.SelectNodes('//*[local-name()="Section"]')).Count -ge 4) { break }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    $manager = New-Object Xml.XmlNamespaceManager($tree.NameTable)
    $manager.AddNamespace('one', $namespace)
    [IO.File]::WriteAllText((Join-Path $Root 'structure-03b-reopened.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $doomed = @($tree.SelectNodes('//*[local-name()="Section"]') | Where-Object { $_.GetAttribute('name') -eq 'Third' })
    if ($doomed.Count -ne 1) { throw "Expected one section named Third, found $($doomed.Count)." }
    $app.DeleteHierarchy($doomed[0].GetAttribute('ID'), [DateTime]::MinValue, $false)
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'structure-04-deleted.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
