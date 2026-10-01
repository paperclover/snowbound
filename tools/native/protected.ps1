param(
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$Section,
    [switch]$Edit
)
# Shows the section file $Section of the run's notebook in OneNote, or with -Edit appends a
# paragraph to the first outline of its first page with one, as an add-in may in an unlocked
# section.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$id = ''
$app.OpenHierarchy((Join-Path (Join-Path $Root 'notebook') $Section), '', [ref]$id, 0)
if (-not $Edit) {
    $app.NavigateTo($id, '', $false)
    Write-Output $id
    exit 0
}
$hierarchy = ''
$app.GetHierarchy($id, 4, [ref]$hierarchy, 1)
[xml]$tree = $hierarchy
$pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
if ($pages.Count -eq 0) { throw 'The section shows no pages: it is still locked.' }
$xml = $null
$children = $null
foreach ($page in $pages) {
    $content = ''
    $app.GetPageContent($page.GetAttribute('ID'), [ref]$content, 0, 1)
    [xml]$xml = $content
    $children = $xml.SelectSingleNode('//*[local-name()="Outline"]/*[local-name()="OEChildren"]')
    if ($children) { break }
}
if (-not $children) { throw 'No page of the section has an outline to type into.' }
$element = $xml.CreateElement('one', 'OE', $namespace)
$text = $xml.CreateElement('one', 'T', $namespace)
[void]$text.AppendChild($xml.CreateCDataSection('Typed by OneNote 2010 under the key.'))
[void]$element.AppendChild($text)
[void]$children.AppendChild($element)
$app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $false)
Start-Sleep -Seconds 15
Write-Output 'edited'
