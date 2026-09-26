param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
function Save-Page([string]$pageId, [string]$name) {
    $content = ''
    $app.GetPageContent($pageId, [ref]$content, 0, 1)
    [IO.File]::WriteAllText((Join-Path $Root $name), $content, [Text.Encoding]::UTF8)
}
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('pictures.one', $notebookId, [ref]$sectionId, 0)
    # Two new pages with OneNote's own title, date and time: one keeps its date, the
    # other has its date and time changed after it was saved once.
    $ids = @()
    foreach ($title in @('Kept date', 'Changed date')) {
        $pageId = ''
        $app.CreateNewPage($sectionId, [ref]$pageId, 0)
        $escaped = [Security.SecurityElement]::Escape($pageId)
        $xml = "<one:Page xmlns:one='$namespace' ID='$escaped'><one:Title><one:OE><one:T>$title</one:T></one:OE></one:Title><one:Outline><one:Position x='72' y='144'/><one:OEChildren><one:OE><one:T>Body of $title</one:T></one:OE></one:OEChildren></one:Outline></one:Page>"
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
        $ids += $pageId
    }
    $app.SyncHierarchy($notebookId)
    Start-Sleep -Seconds 3
    Save-Page $ids[1] 'before.xml'
    $escaped = [Security.SecurityElement]::Escape($ids[1])
    $update = "<one:Page xmlns:one='$namespace' ID='$escaped' dateTime='2024-03-05T14:30:00.000Z'/>"
    [IO.File]::WriteAllText((Join-Path $Root 'update.xml'), $update, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($update, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebookId)
    Start-Sleep -Seconds 3
    Save-Page $ids[1] 'after.xml'
    Save-Page $ids[0] 'kept.xml'
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
