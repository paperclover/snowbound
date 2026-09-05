param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $files = @(Get-ChildItem (Join-Path $Root 'notebook') | Where-Object { $_.Extension -eq '.one' })
    if ($files.Count -ne 1) { throw 'Provide exactly one copied section for the template probe.' }
    $sectionId = ''
    $app.OpenHierarchy($files[0].Name, $notebookId, [ref]$sectionId, 0)
    $pageId = ''
    $app.CreateNewPage($sectionId, [ref]$pageId, 0)
    $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $pageId + '"><one:Title><one:OE><one:T>Native template probe</one:T></one:OE></one:Title></one:Page>'
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
    @{ page = $pageId; operation = 'CreateNewPage with npsDefault, then set title' } | ConvertTo-Json | Set-Content (Join-Path $Root 'expected.json') -Encoding UTF8
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
