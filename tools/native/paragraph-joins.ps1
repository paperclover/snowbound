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
    $mixed = '<b>Bold</b> <i>italic</i> ' + [char]::ConvertFromUtf32(0x1F980) + ' e' + [char]0x0301 + ' <span style="color:#123456">tail</span>'
    $text = '<one:T><![CDATA[' + $mixed + ']]></one:T>'
    $child = '<one:OEChildren><one:OE><one:T>Preserved child</one:T></one:OE></one:OEChildren>'
    $tag = '<one:Tag index="0" completed="true" disabled="false" creationDate="2020-01-02T03:04:05.000Z" completionDate="2020-01-02T04:05:06.000Z"/>'
    $cases = @(
        @{ name = 'Join inherited styles'; body = '<one:OE quickStyleIndex="0"><one:T>Left </one:T></one:OE><one:OE quickStyleIndex="1"><one:T><![CDATA[Right <i>italic</i>]]></one:T></one:OE>' },
        @{ name = 'Join empty left tag'; body = '<one:OE>' + $tag + '<one:T><![CDATA[]]></one:T></one:OE><one:OE>' + $text + '</one:OE>' },
        @{ name = 'Join right tag'; body = '<one:OE><one:T>Left </one:T></one:OE><one:OE>' + $tag + $text + '</one:OE>' },
        @{ name = 'Join both tags'; body = '<one:OE>' + $tag + '<one:T>Left </one:T></one:OE><one:OE>' + $tag + $text + '</one:OE>' },
        @{ name = 'Join both children'; body = '<one:OE><one:T>Left </one:T><one:OEChildren><one:OE><one:T>Left child</one:T></one:OE></one:OEChildren></one:OE><one:OE>' + $text + '<one:OEChildren><one:OE><one:T>Right child</one:T></one:OE></one:OEChildren></one:OE>' }
    )
    $inputs = Join-Path $Root 'inputs'
    New-Item -ItemType Directory -Path $inputs | Out-Null
    foreach ($case in $cases) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 0)
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Fixture task"/><one:QuickStyleDef index="0" name="p" font="Calibri" fontSize="11" fontColor="automatic" highlightColor="automatic"/><one:QuickStyleDef index="1" name="h1" font="Georgia" fontSize="18" bold="true" fontColor="#123456" highlightColor="automatic" spaceBefore="8" spaceAfter="4"/><one:Title><one:OE><one:T>' + $case.name + '</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/><one:OEChildren>' + $case.body + '<one:OE><one:T>Preserved sibling</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
        [IO.File]::WriteAllText((Join-Path $inputs ($case.name + '.xml')), $xml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
        $case.page = $page
    }
    $cases | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $Root 'cases.json') -Encoding UTF8
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
