param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$histories = Get-Content (Join-Path $Root 'notebook\histories.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$inputs = Join-Path $Root 'inputs'
New-Item -ItemType Directory -Path $inputs | Out-Null
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('synthetic.one', $notebookId, [ref]$sectionId, 0)
    foreach ($history in $histories) {
        $pageId = ''
        $app.CreateNewPage($sectionId, [ref]$pageId, 0)
        $state = $history.initial
        $paragraphs = New-Object Collections.ArrayList
        foreach ($paragraph in $state.paragraphs) { [void]$paragraphs.Add($paragraph) }
        $ids = @{}
        $outlineId = ''
        for ($step = -1; $step -lt $history.operations.Count; $step++) {
            if ($step -ge 0) {
                $op = $history.operations[$step]
                switch ($op.kind) {
                    'insert' { $paragraphs.Insert($op.index, $op.paragraph) }
                    'delete' { $paragraphs.RemoveAt($op.index) }
                    'move' { $paragraph = $paragraphs[$op.index]; $paragraphs.RemoveAt($op.index); $paragraphs.Insert($op.to, $paragraph) }
                    'position' { $state.x = $op.x; $state.y = $op.y }
                    'cell' { $state.table[$op.row][$op.column] = $op.text }
                    default { $property = $op.kind; $paragraphs[$op.index].$property = $op.value }
                }
            }
            $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $pageId + '"><one:TagDef index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="History checkbox"/><one:Title><one:OE><one:T>' + $history.title + '</one:T></one:OE></one:Title><one:Outline'
            if ($outlineId) { $xml += ' objectID="' + $outlineId + '"' }
            $xml += '><one:Position x="' + $state.x + '" y="' + $state.y + '"/><one:Size width="480" height="100"/><one:OEChildren>'
            foreach ($paragraph in $paragraphs) {
                $xml += '<one:OE'
                if ($ids.ContainsKey($paragraph.id)) { $xml += ' objectID="' + $ids[$paragraph.id] + '"' }
                $xml += '>'
                if ($paragraph.tag) { $xml += '<one:Tag index="0" completed="false" disabled="false" creationDate="2020-01-02T03:04:05.000Z"/>' }
                $decoration = @()
                if ($paragraph.underline) { $decoration += 'underline' }
                if ($paragraph.strike) { $decoration += 'line-through' }
                if ($decoration.Count -eq 0) { $decoration = @('none') }
                $style = 'font-family:Calibri;font-size:' + $paragraph.font_size + 'pt;font-weight:' + $(if ($paragraph.bold) {'bold'} else {'normal'}) + ';font-style:' + $(if ($paragraph.italic) {'italic'} else {'normal'}) + ';text-decoration:' + ($decoration -join ' ') + ';color:' + $paragraph.color + ';background:' + $paragraph.highlight
                $html = '<span style="' + $style + '">' + [Security.SecurityElement]::Escape($paragraph.text) + '</span>'
                if ($paragraph.link) { $html = '<a href="' + [Security.SecurityElement]::Escape($paragraph.link) + '">' + $html + '</a>' }
                $xml += '<one:T><![CDATA[' + $html + ']]></one:T></one:OE>'
            }
            $xml += '<one:OE><one:Table bordersVisible="true"><one:Columns><one:Column index="0" width="180" isLocked="true"/><one:Column index="1" width="180" isLocked="true"/></one:Columns>'
            foreach ($row in $state.table) {
                $xml += '<one:Row>'
                foreach ($cell in $row) { $xml += '<one:Cell><one:OEChildren><one:OE><one:T><![CDATA[' + [Security.SecurityElement]::Escape($cell) + ']]></one:T></one:OE></one:OEChildren></one:Cell>' }
                $xml += '</one:Row>'
            }
            $xml += '</one:Table></one:OE></one:OEChildren></one:Outline></one:Page>'
            [IO.File]::WriteAllText((Join-Path $inputs ('{0:d6}-{1:d2}.xml' -f $history.seed, ($step + 1))), $xml, [Text.Encoding]::UTF8)
            $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
            $content = ''
            $app.GetPageContent($pageId, [ref]$content, 3, 1)
            [xml]$document = $content
            $manager = New-Object Xml.XmlNamespaceManager($document.NameTable)
            $manager.AddNamespace('one', $namespace)
            $outline = $document.SelectSingleNode('/one:Page/one:Outline', $manager)
            $outlineId = $outline.GetAttribute('objectID')
            $objects = @($outline.SelectNodes('one:OEChildren/one:OE[one:T]', $manager))
            if ($objects.Count -ne $paragraphs.Count) { throw "Native paragraph count changed at seed $($history.seed), step $step." }
            for ($index = 0; $index -lt $paragraphs.Count; $index++) { $ids[$paragraphs[$index].id] = $objects[$index].GetAttribute('objectID') }
            $log = @{ seed = $history.seed; step = $step; page = $pageId; outline = $outlineId } | ConvertTo-Json -Compress
            [IO.File]::AppendAllText((Join-Path $Root 'operations.jsonl'), $log + [Environment]::NewLine, [Text.Encoding]::UTF8)
        }
    }
    $app.SyncHierarchy($notebookId)
    $app.CloseNotebook($notebookId, $false)
    $notebookId = ''
} finally {
    if ($notebookId) { $app.CloseNotebook($notebookId, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
