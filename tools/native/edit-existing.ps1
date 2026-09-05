param([Parameter(Mandatory=$true)][string]$Root,
      [Parameter(Mandatory=$true)][string]$ExpectedText,
      [Parameter(Mandatory=$true)][string]$Text)
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
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $matches = @()
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -gt 1) { throw 'Expected a single demo page.' }
        if ($pages.Count -eq 1) {
            $content = ''
            $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 3, 1)
            [xml]$page = $content
            $matches = @($page.SelectNodes('//*[local-name()="T"]') | Where-Object { $_.InnerText -eq $ExpectedText })
            if ($matches.Count -eq 1) { break }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($matches.Count -ne 1) { throw 'Expected one matching demo text object.' }
    [IO.File]::WriteAllText("$root\before.xml", $content, [Text.Encoding]::UTF8)
    $matches[0].InnerText = [Security.SecurityElement]::Escape($Text)
    $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebook)
} finally {
    try { if ($notebook) { $app.CloseNotebook($notebook, $false) } }
    finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 10
    }
}
