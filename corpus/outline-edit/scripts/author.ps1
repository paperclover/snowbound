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
    $cases = @(
        @{ name='Move leaf down'; keys='{Left}^+{-}!+{Down}'; leaf=$true },
        @{ name='Move subtree down'; keys='{Left}^+{-}!+{Down}' },
        @{ name='Move subtree up'; keys='{Left}^+{-}!+{Up}' },
        @{ name='Indent subtree'; keys='{Left}^+{-}!+{Right}' },
        @{ name='Outdent subtree'; keys='{Left}^+{-}!+{Left}'; nested=$true },
        @{ name='Collapse subtree'; collapse=$true },
        @{ name='Expand subtree'; collapse=$false; collapsed=$true },
        @{ name='Delete leaf'; keys='{Left}^+{-}{Delete}'; leaf=$true },
        @{ name='Delete subtree'; keys='{Left}^+{-}{Delete}' },
        @{ name='Delete only paragraph'; keys='{Left}^+{-}{Delete}'; only=$true; leaf=$true },
        @{ name='Delete outline'; delete='outline' },
        @{ name='Move outline'; position=@{ x=180; y=216 } },
        @{ name='Resize outline'; size=@{ width=144; height=180; isSetByUser=$true } },
        @{ name='Automatic outline size'; size=@{ width=360; height=180; isSetByUser=$false }; fixed=$true }
    )
    $inputs = Join-Path $Root 'inputs'
    [void](New-Item -ItemType Directory -Path $inputs)
    foreach ($case in $cases) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 0)
        $label = 'Target ' + [char]::ConvertFromUtf32(0x1F980) + ' e' + [char]0x0301
        $target = '<one:OE' + $(if ($case.ContainsKey('collapsed')) { ' collapsed="true"' } else { '' }) + '><one:Tag index="0" completed="true" disabled="false" creationDate="2020-01-02T03:04:05.000Z" completionDate="2020-01-02T04:05:06.000Z"/><one:T><![CDATA[<b>' + $label + '</b> <a href="https://example.invalid/outline">Link label</a>]]></one:T>'
        if (-not $case.ContainsKey('leaf')) {
            $target += '<one:OEChildren><one:OE><one:T><![CDATA[<i>Child</i>]]></one:T><one:OEChildren><one:OE><one:T>Grandchild</one:T></one:OE></one:OEChildren></one:OE></one:OEChildren>'
        }
        $target += '</one:OE>'
        $body = if ($case.ContainsKey('only')) { $target } elseif ($case.ContainsKey('nested')) {
            '<one:OE><one:T>Anchor</one:T><one:OEChildren>' + $target + '</one:OEChildren></one:OE><one:OE><one:T>Trailing sibling</one:T></one:OE>'
        } else { '<one:OE><one:T>Anchor</one:T></one:OE>' + $target + '<one:OE><one:T>Trailing sibling</one:T></one:OE>' }
        $size = if ($case.ContainsKey('fixed')) { '<one:Size width="180" height="144" isSetByUser="true"/>' } else { '' }
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Preserved task"/><one:Title><one:OE><one:T>' + $case.name + '</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/>' + $size + '<one:OEChildren>' + $body + '</one:OEChildren></one:Outline><one:Outline><one:Position x="480" y="108"/><one:OEChildren><one:OE><one:T>Other outline</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
        [IO.File]::WriteAllText((Join-Path $inputs ($case.name + '.xml')), $xml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
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
