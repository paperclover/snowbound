param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $matches = @()
    $content = ''
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]') | Where-Object { $_.GetAttribute('name') -eq 'Move leaf down' })
        if ($pages.Count -eq 1) {
            $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 3, 1)
            [xml]$page = $content
            $matches = @($page.SelectNodes('//*[local-name()="T"]') | Where-Object { $_.InnerText -like '*>Rust *' -or $_.InnerText -like 'Rust *' })
            if ($matches.Count -eq 1) { break }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($matches.Count -ne 1) { throw 'Expected exactly one Rust-written paragraph on the Move leaf down page.' }
    [IO.File]::WriteAllText("$Root\before.xml", $content, [Text.Encoding]::UTF8)
    $text = 'Native after Rust ' + [char]::ConvertFromUtf32(0x1F98B) + ' e' + [char]0x0301
    $matches[0].InnerText = [Security.SecurityElement]::Escape($text)
    $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebook)
    [IO.File]::WriteAllText("$Root\native-text.txt", $text, [Text.Encoding]::UTF8)
} finally {
    try { if ($notebook) { $app.CloseNotebook($notebook, $false) } }
    finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 120 -ErrorAction SilentlyContinue
    }
}
