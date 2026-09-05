param([Parameter(Mandatory=$true)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($Root).TrimEnd('\')
if ([IO.Path]::GetDirectoryName($root) -ne 'C:\one-tests\runs') { throw 'Choose a run directly below C:\one-tests\runs.' }
& "$PSScriptRoot\cold-current.ps1" -Root $root
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy("$root\notebook", '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    $page = ''
    $app.CreateNewPage($section, [ref]$page, 2)
    $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:Outline><one:OEChildren><one:OE><one:T>Recoverable deletion marker.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebook)
    $app.CloseNotebook($notebook, $false)
    Copy-Item "$root\notebook" "$root\before" -Recurse
    $app.OpenHierarchy("$root\notebook", '', [ref]$notebook, 0)
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    $hierarchy = ''
    $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $target = @($tree.SelectNodes('//*[local-name()="Page"]') | Where-Object { $_.GetAttribute('name') -eq 'Recoverable deletion marker.' })
    if ($target.Count -ne 1) { throw 'The generated deletion page did not persist.' }
    $app.DeleteHierarchy($target[0].GetAttribute('ID'), [DateTime]::MinValue, $false)
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
    Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 10
}
