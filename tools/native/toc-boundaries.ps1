param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $names = @()
    foreach ($index in 0..79) {
        $name = 'Boundary section {0:d3}.one' -f $index
        $section = ''
        $app.OpenHierarchy($name, $notebookId, [ref]$section, 3)
        $app.SyncHierarchy($notebookId)
        $app.CloseNotebook($notebookId, $false)
        $notebookId = ''
        $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
        $names += $name
    }
    $names | ConvertTo-Json | Set-Content (Join-Path $Root 'expected.json') -Encoding UTF8
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
