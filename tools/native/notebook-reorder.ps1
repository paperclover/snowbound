param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
function Read-Tree {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        $hierarchy = ''
        $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        if (@($tree.SelectNodes('//*[local-name()="Section"]')).Count -ge 4) { return $hierarchy }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'The notebook did not finish loading.'
}
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $hierarchy = Read-Tree
    [IO.File]::WriteAllText((Join-Path $Root 'reorder-00-opened.xml'), $hierarchy, [Text.Encoding]::UTF8)
    [xml]$tree = $hierarchy
    $manager = New-Object Xml.XmlNamespaceManager($tree.NameTable)
    $manager.AddNamespace('one', $namespace)
    $notebookNode = $tree.DocumentElement
    $sections = @($notebookNode.SelectNodes('one:Section', $manager))
    if ($sections.Count -lt 3) { throw "Expected three root sections, found $($sections.Count)." }
    $last = $sections[$sections.Count - 1]
    [void]$notebookNode.RemoveChild($last)
    [void]$notebookNode.InsertBefore($last, $sections[0])
    [IO.File]::WriteAllText((Join-Path $Root 'reorder-01-update.xml'), $tree.OuterXml, [Text.Encoding]::UTF8)
    $app.UpdateHierarchy($tree.OuterXml, 1)
    $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'reorder-02-reordered.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
    Copy-Item -Recurse (Join-Path $Root 'notebook') (Join-Path $Root 'notebook-reordered')
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $hierarchy = Read-Tree
    [IO.File]::WriteAllText((Join-Path $Root 'reorder-03-reopened.xml'), $hierarchy, [Text.Encoding]::UTF8)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
