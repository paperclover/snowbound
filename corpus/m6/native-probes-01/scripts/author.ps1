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
    $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $pageId + '">' + $body + '</one:Page>'
    [IO.File]::WriteAllText((Join-Path $inputs 'format-probes.xml'), $xml, [Text.Encoding]::UTF8)
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
    $cases | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $Root 'expected.json') -Encoding UTF8
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
