param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\page-lifecycle.ps1" -Root $Root -CloneHost $CloneHost -CaptureOnly before
$request = Get-Content (Join-Path $Root 'removal.json') -Raw | ConvertFrom-Json
$app = New-Object -ComObject OneNote.Application
try {
    $notebook = ''
    $section = ''
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $app.OpenHierarchy('Lifecycle.one', $notebook, [ref]$section, 0)
    $hierarchy = ''
    $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
    if ($pages.Count -ne 9) { throw 'The removal fixture must have nine original pages.' }
    $ids = @()
    foreach ($index in $request.selected) {
        if ($index -lt 0 -or $index -ge $pages.Count) { throw 'Choose an original page in the fixture.' }
        $ids += $pages[$index].GetAttribute('ID')
    }
    $ids | ConvertTo-Json | Set-Content (Join-Path $Root 'removed-ids.json') -Encoding UTF8
    foreach ($id in $ids) {
        $app.DeleteHierarchy($id, [DateTime]::MinValue, [bool]$request.permanent)
    }
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
& "$PSScriptRoot\page-lifecycle.ps1" -Root $Root -CloneHost $CloneHost -CaptureOnly after
