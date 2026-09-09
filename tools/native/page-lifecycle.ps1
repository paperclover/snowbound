param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
$section = ''
$pages = [ordered]@{}
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'

function Save-Phase([string]$Name) {
    $destination = Join-Path $Root $Name
    [void](New-Item -ItemType Directory -Path $destination)
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $previous = ''
    $stableSince = [DateTime]::UtcNow
    do {
        $hierarchy = ''
        $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $contents = @()
        foreach ($node in $tree.SelectNodes('//*[local-name()="Page"]')) {
            $content = ''
            $app.GetPageContent($node.GetAttribute('ID'), [ref]$content, 1, 1)
            $contents += $content
        }
        $signature = $hierarchy + [String]::Join('|', $contents)
        if ($signature -ne $previous) {
            $previous = $signature
            $stableSince = [DateTime]::UtcNow
        }
        if (([DateTime]::UtcNow - $stableSince).TotalSeconds -ge 2) { break }
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Native page content did not settle.' }
        Start-Sleep -Milliseconds 250
    } while ($true)
    [IO.File]::WriteAllText((Join-Path $destination 'hierarchy.xml'), $hierarchy, [Text.Encoding]::UTF8)
    for ($index = 0; $index -lt $contents.Count; $index++) {
        [IO.File]::WriteAllText((Join-Path $destination ('page-{0:d3}.xml' -f $index)), $contents[$index], [Text.Encoding]::UTF8)
    }
    $app.SyncHierarchy($notebook)
    $app.CloseNotebook($notebook, $false)
    Copy-Item (Join-Path $Root 'notebook') (Join-Path $destination 'notebook') -Recurse
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$script:notebook, 0)
    $app.OpenHierarchy('Lifecycle.one', $notebook, [ref]$script:section, 0)
    $beforePages = @($tree.SelectNodes('//*[local-name()="Page"]'))
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $reopened = ''
        $app.GetHierarchy($section, 4, [ref]$reopened, 1)
        [xml]$current = $reopened
        $afterPages = @($current.SelectNodes('//*[local-name()="Page"]'))
        if ($beforePages.Count -eq $afterPages.Count) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    [IO.File]::WriteAllText((Join-Path $destination 'reopened-hierarchy.xml'), $reopened, [Text.Encoding]::UTF8)
    if ($beforePages.Count -ne $afterPages.Count) { throw 'Native reopen changed the page count.' }
    for ($index = 0; $index -lt $beforePages.Count; $index++) {
        foreach ($attribute in @('name', 'pageLevel')) {
            if ($beforePages[$index].GetAttribute($attribute) -ne $afterPages[$index].GetAttribute($attribute)) {
                throw 'Native reopen changed ordered page metadata.'
            }
        }
        foreach ($key in @($pages.Keys)) {
            if ($pages[$key] -eq $beforePages[$index].GetAttribute('ID')) {
                $pages[$key] = $afterPages[$index].GetAttribute('ID')
            }
        }
    }
    $pages | ConvertTo-Json | Set-Content (Join-Path $destination 'pages.json') -Encoding UTF8
    Write-Output $Name
}

function Set-Title([string]$Page, [string]$Title) {
    Write-Output ('Set title ' + $Page + ' to ' + $Title)
    $value = [Security.SecurityElement]::Escape($Title)
    $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $Page + '"><one:Title><one:OE><one:T>' + $value + '</one:T></one:OE></one:Title></one:Page>'
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
}

function Set-Order([string[]]$Names, [int[]]$Levels) {
    $xml = '<one:Section xmlns:one="' + $namespace + '" ID="' + $section + '">'
    for ($index = 0; $index -lt $Names.Count; $index++) {
        $xml += '<one:Page ID="' + $pages[$Names[$index]] + '" pageLevel="' + $Levels[$index] + '"/>'
    }
    $xml += '</one:Section>'
    $app.UpdateHierarchy($xml, 1)
}

try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $app.OpenHierarchy('Lifecycle.one', $notebook, [ref]$section, 3)
    foreach ($style in 0..2) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, $style)
        $pages['blank' + $style] = $page
    }
    Save-Phase '01-blank'
    foreach ($name in @('parent', 'child', 'grandchild', 'duplicate', 'trailing', 'automatic')) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 2)
        $pages[$name] = $page
        if ($name -ne 'automatic') {
            Set-Title $page $(if ($name -in @('parent', 'duplicate')) { 'Same title' } else { $name })
        }
        $body = [Security.SecurityElement]::Escape('Body ' + $name + ' ' + [char]::ConvertFromUtf32(0x1F980) + ' e' + [char]0x0301)
        $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $page + '"><one:Outline><one:OEChildren><one:OE><one:T><![CDATA[<b>' + $body + '</b> <i>preserved</i>]]></one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    }
    $pages | ConvertTo-Json | Set-Content (Join-Path $Root 'pages.json') -Encoding UTF8
    Save-Phase '02-authored'
    Set-Title $pages['duplicate'] ('Renamed ' + [char]::ConvertFromUtf32(0x1F98B) + ' e' + [char]0x0301)
    Set-Title $pages['trailing'] ''
    Save-Phase '03-renamed'
    Set-Order @('blank0','blank1','blank2','parent','child','grandchild','duplicate','trailing','automatic') @(1,1,1,1,2,3,1,1,1)
    Save-Phase '04-nested'
    Set-Order @('blank0','blank1','blank2','duplicate','parent','child','grandchild','trailing','automatic') @(1,1,1,1,1,2,3,1,1)
    Save-Phase '05-reordered'
    $app.DeleteHierarchy($pages['parent'], [DateTime]::MinValue, $false)
    Save-Phase '06-deleted-parent'
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
