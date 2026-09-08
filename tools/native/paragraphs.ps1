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
    $cases = @(
        @{ name = 'Split middle'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE>' + $text + '</one:OE>' },
        @{ name = 'Split style boundary'; keys = '{Left}{Right 4}{Enter}'; body = '<one:OE>' + $text + '</one:OE>' },
        @{ name = 'Split start'; keys = '{Left}{Enter}'; body = '<one:OE>' + $text + '</one:OE>' },
        @{ name = 'Split end'; keys = '{Right}{Enter}'; body = '<one:OE>' + $text + '</one:OE>' },
        @{ name = 'Split empty'; keys = '{Left}{Enter}'; body = '<one:OE><one:T><![CDATA[]]></one:T></one:OE>' },
        @{ name = 'Split parent'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE>' + $text + $child + '</one:OE>' },
        @{ name = 'Split child'; keys = '{Left}{Right 2}{Enter}'; nested = $true; body = '<one:OE><one:T>Preserved parent</one:T><one:OEChildren><one:OE>' + $text + '</one:OE></one:OEChildren></one:OE>' },
        @{ name = 'Split bullet'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:List><one:Bullet bullet="3" fontSize="11"/></one:List>' + $text + $child + '</one:OE>' },
        @{ name = 'Split number'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:List><one:Number numberSequence="1" numberFormat="##." restartNumberingAt="3" font="Calibri" fontSize="11"/></one:List>' + $text + '</one:OE>' },
        @{ name = 'Split tag'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:Tag index="0" completed="true" disabled="false" creationDate="2020-01-02T03:04:05.000Z" completionDate="2020-01-02T04:05:06.000Z"/>' + $text + '</one:OE>' },
        @{ name = 'Split cell'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:Table bordersVisible="true"><one:Columns><one:Column index="0" width="180" isLocked="true"/><one:Column index="1" width="180"/></one:Columns><one:Row><one:Cell><one:OEChildren><one:OE>' + $text + '</one:OE></one:OEChildren></one:Cell><one:Cell><one:OEChildren><one:OE><one:T>Preserved cell</one:T></one:OE></one:OEChildren></one:Cell></one:Row></one:Table></one:OE>' },
        @{ name = 'Split soft break'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:T><![CDATA[Bold<br>' + "`n" + 'soft break]]></one:T></one:OE>' },
        @{ name = 'Split before hyperlink'; keys = '{Left}{Right 2}{Enter}'; body = '<one:OE><one:T><![CDATA[Before <a href="https://example.invalid/paragraph">Link label</a> tail]]></one:T></one:OE>' }
    )
    $inputs = Join-Path $Root 'inputs'
    New-Item -ItemType Directory -Path $inputs | Out-Null
    foreach ($case in $cases) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 0)
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Fixture task"/><one:Title><one:OE><one:T>' + $case.name + '</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/><one:OEChildren>' + $case.body + '<one:OE><one:T>Preserved sibling</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
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
