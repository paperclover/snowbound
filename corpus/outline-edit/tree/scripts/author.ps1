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
        @{ name='Indent first subtree'; shape='first'; keys='{Left}^+{-}!+{Right}' },
        @{ name='Outdent first group'; shape='group'; keys='{Left}^+{-}!+{Left}' },
        @{ name='Delete only grouped subtree'; shape='group'; keys='{Left}^+{-}{Delete}' },
        @{ name='Delete unindented sibling after group'; shape='tail'; keys='{Left}^+{-}{Delete}' },
        @{ name='Delete cell subtree'; shape='cell'; keys='{Left}^+{-}{Delete}' },
        @{ name='Delete sole cell paragraph'; shape='cell-only'; keys='{Left}^+{-}{Delete}' },
        @{ name='Move numbered subtree down'; shape='number'; keys='{Left}^+{-}!+{Down}' },
        @{ name='Delete numbered subtree'; shape='number'; keys='{Left}^+{-}{Delete}' },
        @{ name='Indent bullet subtree'; shape='bullet'; keys='{Left}^+{-}!+{Right}' },
        @{ name='Outdent child across indentation gap'; shape='gap'; keys='{Left}^+{-}!+{Left}' },
        @{ name='Move cell subtree down'; shape='cell'; keys='{Left}^+{-}!+{Down}' }
    )
    $inputs = Join-Path $Root 'inputs'
    [void](New-Item -ItemType Directory -Path $inputs)
    foreach ($case in $cases) {
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 0)
        $label = 'Target ' + [char]::ConvertFromUtf32(0x1F980) + ' e' + [char]0x0301
        $list = if ($case.shape -eq 'number') { '<one:List><one:Number numberSequence="1" numberFormat="##." font="Calibri" fontSize="11"/></one:List>' } elseif ($case.shape -eq 'bullet') { '<one:List><one:Bullet bullet="3" fontSize="11"/></one:List>' } else { '' }
        $child = if ($case.shape -in @('cell-only','tail')) { '' } else { '<one:OEChildren><one:OE><one:T><![CDATA[<i>Child</i>]]></one:T><one:OEChildren><one:OE><one:T>Grandchild</one:T></one:OE></one:OEChildren></one:OE></one:OEChildren>' }
        $target = '<one:OE><one:Tag index="0" completed="true" disabled="false" creationDate="2020-01-02T03:04:05.000Z" completionDate="2020-01-02T04:05:06.000Z"/>' + $list + '<one:T><![CDATA[<b>' + $label + '</b> <a href="https://example.invalid/tree">Link label</a>]]></one:T>' + $child + '</one:OE>'
        $anchor = '<one:OE>' + $list + '<one:T>Anchor</one:T></one:OE>'
        $trailing = '<one:OE>' + $list + '<one:T>Trailing sibling</one:T></one:OE>'
        $body = switch ($case.shape) {
            'first' { '<one:OEChildren>' + $target + $trailing + '</one:OEChildren>' }
            'group' { '<one:OEChildren indent="2">' + $target + '</one:OEChildren><one:OEChildren>' + $trailing + '</one:OEChildren>' }
            'tail' { '<one:OEChildren indent="2">' + $anchor + '</one:OEChildren><one:OEChildren>' + $target + '</one:OEChildren>' }
            'gap' { '<one:OEChildren><one:OE><one:T>Anchor</one:T><one:OEChildren indent="2">' + $target + '<one:OE><one:T>Other child</one:T></one:OE></one:OEChildren></one:OE>' + $trailing + '</one:OEChildren>' }
            { $_ -in @('cell','cell-only') } {
                $tail = if ($case.shape -eq 'cell-only') { '' } else { '<one:OE><one:T>Cell tail</one:T></one:OE>' }
                '<one:OEChildren><one:OE><one:Table bordersVisible="true"><one:Columns><one:Column index="0" width="240" isLocked="true"/><one:Column index="1" width="180" isLocked="true"/></one:Columns><one:Row><one:Cell><one:OEChildren>' + $target + $tail + '</one:OEChildren></one:Cell><one:Cell><one:OEChildren><one:OE><one:T>Other cell</one:T></one:OE></one:OEChildren></one:Cell></one:Row></one:Table></one:OE></one:OEChildren>'
            }
            default { '<one:OEChildren>' + $anchor + $target + $trailing + '</one:OEChildren>' }
        }
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Preserved task"/><one:Title><one:OE><one:T>' + $case.name + '</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/>' + $body + '</one:Outline><one:Outline><one:Position x="540" y="108"/><one:OEChildren><one:OE><one:T>Other outline</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
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
