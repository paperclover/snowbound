$app = New-Object -ComObject OneNote.Application
$notebook = ''
$app.OpenHierarchy('C:\one-tests\runs\capture\notebook', '', [ref]$notebook, 0)
$section = ''
$app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
$hierarchy = ''
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
    $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
    if ($pages.Count -eq 17) { break }
    Start-Sleep -Milliseconds 200
} while ([DateTime]::UtcNow -lt $deadline)
Write-Output ('pages=' + $pages.Count)
Write-Output $hierarchy
foreach ($page in $tree.SelectNodes('//*[local-name()="Page"]')) {
    $content = ''
    $app.GetPageContent($page.GetAttribute('ID'), [ref]$content, 0, 1)
    [xml]$xml = $content
    $texts = @($xml.SelectNodes('//*[local-name()="Outline"]//*[local-name()="T"]') | ForEach-Object { $_.InnerText })
    @{ name=$page.GetAttribute('name'); texts=$texts; tags=@($xml.SelectNodes('//*[local-name()="Tag"]')).Count; lists=@($xml.SelectNodes('//*[local-name()="List"]')).Count } | ConvertTo-Json -Compress
}
$app.CloseNotebook($notebook, $false)
