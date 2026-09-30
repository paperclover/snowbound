param([Parameter(Mandatory=$true)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$notebook = Join-Path $Root 'notebook'
$app = New-Object -ComObject OneNote.Application
try {
    $hierarchy = ''
    $app.GetHierarchy('', 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $page = $tree.SelectSingleNode('//*[local-name()="Notebook"][@path="' + $notebook + '"]//*[local-name()="Page"]')
    if (-not $page) { throw 'The notebook has no page open.' }
    $content = ''
    $app.GetPageContent($page.GetAttribute('ID'), [ref]$content, 0, 1)
    [IO.File]::WriteAllText((Join-Path $Root 'before.xml'), $content, [Text.Encoding]::UTF8)
    [xml]$xml = $content
    # OneNote edits the tagged paragraphs: new text on one, the check box of another cleared.
    foreach ($oe in $xml.SelectNodes('//*[local-name()="OE"]')) {
        $text = $oe.SelectSingleNode('*[local-name()="T"]')
        if (-not $text) { continue }
        if ($text.InnerText -eq 'Launch day') { $text.InnerXml = '<![CDATA[Launch day, moved to Friday]]>' }
        if ($text.InnerText -eq 'Checked off') {
            foreach ($tag in $oe.SelectNodes('*[local-name()="Tag"]')) { $tag.SetAttribute('completed', 'false') }
        }
    }
    [IO.File]::WriteAllText((Join-Path $Root 'update.xml'), $xml.OuterXml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $false)
    $id = $tree.SelectSingleNode('//*[local-name()="Notebook"][@path="' + $notebook + '"]').GetAttribute('ID')
    $app.SyncHierarchy($id)
    Start-Sleep -Seconds 5
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
# What OneNote left of Snowbound's folder: its attribute, and each file with its hash.
$folder = Get-Item -LiteralPath (Join-Path $notebook '.snowbound') -Force
$files = @(Get-ChildItem -LiteralPath $folder.FullName -Recurse -Force | ForEach-Object {
    $hash = $null
    if (-not $_.PSIsContainer) { $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLower() }
    @{ path = $_.FullName.Substring($folder.FullName.Length + 1).Replace('\', '/');
       attributes = $_.Attributes.ToString(); sha256 = $hash }
})
@{ attributes = $folder.Attributes.ToString(); files = $files;
   notebook = @(Get-ChildItem -LiteralPath $notebook -Force | ForEach-Object { $_.Name }) } |
    ConvertTo-Json -Depth 4 | Set-Content (Join-Path $Root 'sidecar.json') -Encoding UTF8
