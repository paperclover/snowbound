param([Parameter(Mandatory=$true)][string]$Root, [string]$CloneHost = '')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    foreach ($name in @('Removed.one', 'Retired.one')) {
        $section = ''
        $app.OpenHierarchy($name, $notebook, [ref]$section, 3)
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 2)
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:Outline><one:OEChildren><one:OE><one:T>' + $name + ' fixture.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    }
    $app.SyncHierarchy($notebook)
    $app.CloseNotebook($notebook, $false)
    Copy-Item (Join-Path $Root 'notebook') (Join-Path $Root 'before') -Recurse
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    foreach ($name in @('Removed.one', 'Retired.one')) {
        $section = ''
        $app.OpenHierarchy($name, $notebook, [ref]$section, 0)
        $app.DeleteHierarchy($section, [DateTime]::MinValue, ($name -eq 'Retired.one'))
    }
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
