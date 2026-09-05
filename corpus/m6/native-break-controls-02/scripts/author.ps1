param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$fixture = Get-Content (Join-Path $Root 'notebook\fixture.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$inputs = Join-Path $Root 'inputs'
New-Item -ItemType Directory -Path $inputs | Out-Null
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sections = @{}
    foreach ($path in $fixture.sections) {
        $parts = $path.Split('/')
        if (@($parts | Where-Object { $_ -in @('', '.', '..') -or $_.Contains('\') -or $_.Contains(':') }).Count) { throw 'Invalid fixture section path.' }
        $parent = $notebookId
        for ($index = 0; $index -lt $parts.Count; $index++) {
            $id = ''
            $app.OpenHierarchy($parts[$index], $parent, [ref]$id, $(if ($index -eq $parts.Count - 1) {3} else {2}))
            $parent = $id
        }
        $sections[$path] = $id
    }
    $index = 0
    foreach ($item in $fixture.pages) {
        $pageId = ''
        $app.CreateNewPage($sections[$item.section], [ref]$pageId, 0)
        [xml]$xml = $item.xml.Replace('__ASSETS__', [Security.SecurityElement]::Escape((Join-Path $Root 'notebook\fixture-data')))
        $xml.DocumentElement.SetAttribute('ID', $pageId)
        [IO.File]::WriteAllText((Join-Path $inputs ('page-{0:d3}.xml' -f $index)), $xml.OuterXml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $true)
        $index++
    }
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
