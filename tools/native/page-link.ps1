param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('links.one', $notebookId, [ref]$sectionId, 0)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $page = $null
    $pages = @()
    do {
        $hierarchy = ''
        $app.GetHierarchy($sectionId, 4, [ref]$hierarchy, 1)
        [xml]$tree = $hierarchy
        $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
        if ($pages.Count -eq 1) {
            $content = ''
            $app.GetPageContent($pages[0].GetAttribute('ID'), [ref]$content, 3, 1)
            [xml]$page = $content
            if (@($page.SelectNodes('//*[local-name()="OE"]')).Count -ge 1) { break }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($pages.Count -ne 1) { throw 'Expected one page in the section.' }
    [IO.File]::WriteAllText((Join-Path $Root 'before.xml'), $content, [Text.Encoding]::UTF8)
    $targetId = ''
    $app.CreateNewPage($sectionId, [ref]$targetId, 0)
    $target = '<one:Page xmlns:one="' + $namespace + '" ID="' + $targetId + '"><one:Title><one:OE><one:T><![CDATA[Link target]]></one:T></one:OE></one:Title><one:Outline><one:Position x="36" y="86"/><one:Size width="300" height="30"/><one:OEChildren><one:OE><one:T><![CDATA[Target paragraph]]></one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
    $app.UpdatePageContent($target, [DateTime]::MinValue, 1, $true)
    $targetContent = ''
    $app.GetPageContent($targetId, [ref]$targetContent, 3, 1)
    [xml]$targetPage = $targetContent
    $targetParagraph = $targetPage.SelectSingleNode('//*[local-name()="Outline"]/*[local-name()="OEChildren"]/*[local-name()="OE"]').GetAttribute('objectID')
    $pageLink = ''
    $app.GetHyperlinkToObject($targetId, '', [ref]$pageLink)
    $paragraphLink = ''
    $app.GetHyperlinkToObject($targetId, $targetParagraph, [ref]$paragraphLink)
    $sectionLink = ''
    $app.GetHyperlinkToObject($sectionId, '', [ref]$sectionLink)
    @{ page = $pageLink; paragraph = $paragraphLink; section = $sectionLink; target = $targetId; target_paragraph = $targetParagraph } | ConvertTo-Json | Set-Content (Join-Path $Root 'links.json') -Encoding UTF8
    $children = $page.SelectSingleNode('//*[local-name()="Outline"]/*[local-name()="OEChildren"]')
    foreach ($entry in @(@('Page link', $pageLink), @('Paragraph link', $paragraphLink), @('Section link', $sectionLink))) {
        $oe = $page.CreateElement('one', 'OE', $namespace)
        $t = $page.CreateElement('one', 'T', $namespace)
        [void]$t.AppendChild($page.CreateCDataSection('<a href="' + [Security.SecurityElement]::Escape($entry[1]) + '">' + $entry[0] + '</a>'))
        [void]$oe.AppendChild($t)
        [void]$children.AppendChild($oe)
    }
    [IO.File]::WriteAllText((Join-Path $Root 'update.xml'), $page.OuterXml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $false)
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
