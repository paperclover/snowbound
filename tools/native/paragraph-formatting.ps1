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
    $page = ''
    $app.CreateNewPage($section, [ref]$page, 0)
    $cases = @(
        @{ name = 'Default'; attributes = '' },
        @{ name = 'Explicit zero'; attributes = ' alignment="left" RTL="false" spaceBefore="0" spaceAfter="0" spaceBetween="0"' },
        @{ name = 'Center spaced'; attributes = ' alignment="center" spaceBefore="12" spaceAfter="6" spaceBetween="20"' },
        @{ name = 'Right RTL'; attributes = ' alignment="right" RTL="true" spaceBefore="3" spaceAfter="9" spaceBetween="18"' },
        @{ name = 'Style inherited'; attributes = ' quickStyleIndex="1"' },
        @{ name = 'Style overridden'; attributes = ' quickStyleIndex="1" alignment="center" spaceBefore="0" spaceAfter="2" spaceBetween="16"' }
    )
    $body = '<one:QuickStyleDef index="0" name="p" font="Calibri" fontSize="11" fontColor="automatic" highlightColor="automatic"/><one:QuickStyleDef index="1" name="h1" font="Georgia" fontSize="18" bold="true" fontColor="#123456" highlightColor="automatic" spaceBefore="8" spaceAfter="4"/>'
    $body += '<one:Title><one:OE><one:T>Paragraph formatting</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/><one:OEChildren>'
    foreach ($case in $cases) {
        $body += '<one:OE' + $case.attributes + '><one:T><![CDATA[' + $case.name + ': <b>bold</b> <i>italic</i> plain]]></one:T></one:OE>'
    }
    $body += '</one:OEChildren></one:Outline>'
    $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '">' + $body + '</one:Page>'
    $inputs = Join-Path $Root 'inputs'
    New-Item -ItemType Directory -Path $inputs | Out-Null
    [IO.File]::WriteAllText((Join-Path $inputs 'paragraph-formatting.xml'), $xml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
