param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$inputs = Join-Path $Root 'inputs'
New-Item -ItemType Directory -Path $inputs | Out-Null
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('synthetic.one', $notebookId, [ref]$sectionId, 0)
    $cases = @(
        @{ name = 'Black highlight control'; html = '<span style="background:black">Black highlight, automatic text</span>' },
        @{ name = 'White on black'; html = '<span style="background:black;color:white">White text on black highlight</span>' },
        @{ name = 'Black on black'; html = '<span style="background:black;color:black">Black text on black highlight</span>' },
        @{ name = 'Partial black highlight'; html = '<b>Before <span style="background:black">middle</span> after</b>' },
        @{ name = 'Yellow control'; html = '<span style="background:yellow">Yellow highlight, automatic text</span>' },
        @{ name = 'Character styles'; html = 'Plain <b>bold</b> <i>italic</i> <u>underline</u> <s>strike</s> <sup>super</sup> <sub>sub</sub> <span style="font-family:Consolas;font-size:18pt;color:#804020;background:#00ffff">font and color</span>' },
        @{ name = 'Unicode runs'; html = 'BMP <b>' + [char]::ConvertFromUtf32(0x1F600) + 'e' + [char]0x0301 + '</b> ' + [char]0x6771 + [char]0x4EAC + ' <i>' + [char]0x0645 + [char]0x0631 + [char]0x062D + [char]0x0628 + [char]0x0627 + '</i>' },
        @{ name = 'Link runs'; html = 'Before <a href="https://example.invalid/fixture?x=1&amp;y=2"><b>Link label</b></a> after' }
    )
    $pageId = ''
    $app.CreateNewPage($sectionId, [ref]$pageId, 0)
    $body = '<one:Title><one:OE><one:T><![CDATA[Native feature probes]]></one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/><one:Size width="480" height="400"/><one:OEChildren>'
    foreach ($case in $cases) { $body += '<one:OE><one:T><![CDATA[' + $case.html + ']]></one:T></one:OE>' }
    $body += '</one:OEChildren></one:Outline>'
    $body = '<one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Fixture checkbox"/><one:TagDef index="1" type="90" symbol="13" fontColor="#804020" highlightColor="#00ffff" name="Fixture custom tag"/><one:QuickStyleDef index="0" name="h1" font="Calibri" fontSize="20" bold="true" fontColor="#123456" highlightColor="automatic" spaceBefore="8" spaceAfter="4"/>' + $body
    $body += '<one:Outline><one:Position x="72" y="540"/><one:Size width="480" height="600"/><one:OEChildren>'
    $body += '<one:OE quickStyleIndex="0" alignment="right"><one:T><![CDATA[Named paragraph style]]></one:T></one:OE>'
    $body += '<one:OE><one:Tag index="0" completed="false" disabled="false" creationDate="2020-01-02T03:04:05.000Z"/><one:Tag index="1" completed="true" disabled="false" creationDate="2020-01-02T03:04:05.000Z" completionDate="2020-01-02T04:05:06.000Z"/><one:T><![CDATA[Two tags on this paragraph]]></one:T></one:OE>'
    $body += '<one:OE><one:List><one:Bullet bullet="3" fontSize="11"/></one:List><one:T><![CDATA[Outer bullet]]></one:T><one:OEChildren><one:OE><one:List><one:Bullet bullet="13" fontSize="11"/></one:List><one:T><![CDATA[Nested bullet]]></one:T></one:OE></one:OEChildren></one:OE>'
    $body += '<one:OE><one:List><one:Number numberSequence="1" numberFormat="##." restartNumberingAt="3" font="Calibri" fontSize="11"/></one:List><one:T><![CDATA[Numbered item three]]></one:T></one:OE><one:OE><one:List><one:Number numberSequence="1" numberFormat="##." font="Calibri" fontSize="11"/></one:List><one:T><![CDATA[Numbered item four]]></one:T></one:OE>'
    $body += '<one:OE><one:Table bordersVisible="true"><one:Columns><one:Column index="0" width="120" isLocked="true"/><one:Column index="1" width="180"/></one:Columns>'
    foreach ($row in 0..2) {
        $body += '<one:Row>'
        foreach ($column in 0..1) {
            $body += '<one:Cell shadingColor="#ffff00"><one:OEChildren><one:OE><one:T><![CDATA[Cell ' + $row + ',' + $column + ']]></one:T></one:OE></one:OEChildren></one:Cell>'
        }
        $body += '</one:Row>'
    }
    $body += '</one:Table></one:OE></one:OEChildren></one:Outline>'

    $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $pageId + '">' + $body + '</one:Page>'
    [IO.File]::WriteAllText((Join-Path $inputs 'format-probes.xml'), $xml, [Text.Encoding]::UTF8)
    [xml]$doc = $xml
    $results = @()
    $n = 0
    $table = $doc.Page.Outline[1].OEChildren.OE[-1].OuterXml
    $variants = @($table, $table.Replace(' shadingColor="#ffff00"', ''), $table.Replace(' isLocked="true"', '').Replace(' shadingColor="#ffff00"', ''), $table.Replace(' isLocked="true"', ''))
    $parts = @($variants | ForEach-Object { '<one:Outline><one:OEChildren>' + $_ + '</one:OEChildren></one:Outline>' })
    foreach ($part in $parts) {
        $testId = ''
        $app.CreateNewPage($sectionId, [ref]$testId, 0)
        $testXml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $testId + '">' + ($doc.Page.TagDef | ForEach-Object { $_.OuterXml }) + $doc.Page.QuickStyleDef.OuterXml + '<one:Title><one:OE><one:T>Probe ' + $n + '</one:T></one:OE></one:Title>' + $part + '</one:Page>'
        [IO.File]::WriteAllText((Join-Path $inputs ('probe-' + $n + '.xml')), $testXml, [Text.Encoding]::UTF8)
        try {
            $app.UpdatePageContent($testXml, [DateTime]::MinValue, 1, $true)
            $results += @{ probe = $n; result = 'accepted' }
        } catch { $results += @{ probe = $n; result = $_.Exception.Message } }
        $n++
    }
    $results | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $Root 'expected.json') -Encoding UTF8
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
