param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost, [switch]$Remote)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $Remote) { & "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost }
$app = New-Object -ComObject OneNote.Application
$notebook = ''
$ns = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    if (-not $Remote) {
        $cases = @(
            @{ name = 'Split remote prefix'; operation = 'split'; change = 'prefix' },
            @{ name = 'Split remote boundary'; operation = 'split'; change = 'boundary' },
            @{ name = 'Split remote child'; operation = 'split'; change = 'child' },
            @{ name = 'Split remote list'; operation = 'split'; change = 'list' },
            @{ name = 'Split remote tag'; operation = 'split'; change = 'tag' },
            @{ name = 'Split remote format'; operation = 'split'; change = 'format' },
            @{ name = 'Split remote sibling'; operation = 'split'; change = 'sibling' },
            @{ name = 'Join remote prefix'; operation = 'join'; change = 'prefix' },
            @{ name = 'Join left boundary'; operation = 'join'; change = 'boundary' },
            @{ name = 'Join right boundary'; operation = 'join'; change = 'right-boundary' },
            @{ name = 'Join remote child'; operation = 'join'; change = 'child' },
            @{ name = 'Join remote list'; operation = 'join'; change = 'list' },
            @{ name = 'Join remote tag'; operation = 'join'; change = 'tag' },
            @{ name = 'Join remote format'; operation = 'join'; change = 'format' },
            @{ name = 'Join remote sibling'; operation = 'join'; change = 'sibling' },
            @{ name = 'Join empty adoption'; operation = 'join'; change = 'adoption' }
        )
        foreach ($case in $cases) {
            $page = ''
            $app.CreateNewPage($section, [ref]$page, 0)
            $left = if ($case.change -eq 'adoption') { '' } else { 'ab' + [char]::ConvertFromUtf32(0x1F980) + 'cd' }
            $body = '<one:OE><one:T><![CDATA[<b>' + $left + '</b>]]></one:T></one:OE>'
            if ($case.operation -eq 'join') { $body += '<one:OE><one:T><![CDATA[<b>Right</b>]]></one:T></one:OE>' }
            $body += '<one:OE><one:T>Preserved sibling</one:T></one:OE>'
            $xml = '<one:Page xmlns:one="' + $ns + '" ID="' + $page + '"><one:Title><one:OE><one:T>' + $case.name + '</one:T></one:OE></one:Title><one:Outline><one:Position x="72" y="108"/><one:OEChildren>' + $body + '</one:OEChildren></one:Outline></one:Page>'
            $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
        }
        $cases | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $Root 'cases.json') -Encoding UTF8
    } else {
        $cases = Get-Content (Join-Path $Root 'cases.json') -Raw | ConvertFrom-Json
        $hierarchy = ''
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        do {
            $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
            [xml]$tree = $hierarchy
            $manager = New-Object Xml.XmlNamespaceManager($tree.NameTable)
            $manager.AddNamespace('one', $ns)
            if (@($tree.SelectNodes('//one:Page', $manager)).Count -eq $cases.Count + 1) { break }
            Start-Sleep -Milliseconds 200
        } while ([DateTime]::UtcNow -lt $deadline)
        $updates = Join-Path $Root 'remote-updates'
        [void](New-Item -ItemType Directory -Path $updates -Force)
        foreach ($case in $cases) {
            $page = @($tree.SelectNodes('//one:Page', $manager) | Where-Object { $_.GetAttribute('name') -eq $case.name })
            if ($page.Count -ne 1) { throw ('Expected one native control page: ' + $case.name) }
            $content = ''
            $app.GetPageContent($page[0].GetAttribute('ID'), [ref]$content, 0, 1)
            [xml]$xml = $content
            $manager = New-Object Xml.XmlNamespaceManager($xml.NameTable)
            $manager.AddNamespace('one', $ns)
            $outline = $xml.SelectSingleNode('/one:Page/one:Outline/one:OEChildren', $manager)
            $paragraphs = @($outline.SelectNodes('one:OE', $manager))
            $left = $paragraphs[0]
            $target = if ($case.operation -eq 'join') { $paragraphs[1] } else { $left }
            switch ($case.change) {
                'prefix' {
                    $text = $left.SelectSingleNode('one:T', $manager)
                    $text.InnerText = 'X' + $text.InnerText
                    if ($case.operation -eq 'join') {
                        $text = $target.SelectSingleNode('one:T', $manager)
                        $text.InnerText += 'Y'
                    }
                }
                'boundary' {
                    $text = $left.SelectSingleNode('one:T', $manager)
                    if ($case.operation -eq 'split') {
                        if (-not $text.InnerText.Contains('ab')) { throw 'The split boundary control lost its text' }
                        $text.InnerText = $text.InnerText.Replace('ab', 'abX')
                    } else { $text.InnerText += 'X' }
                }
                'right-boundary' {
                    $text = $target.SelectSingleNode('one:T', $manager)
                    $text.InnerText = 'X' + $text.InnerText
                }
                'child' {
                    $children = $xml.CreateElement('one', 'OEChildren', $ns)
                    $child = $xml.CreateElement('one', 'OE', $ns)
                    $text = $xml.CreateElement('one', 'T', $ns)
                    $text.InnerText = 'Native child'
                    [void]$child.AppendChild($text)
                    [void]$children.AppendChild($child)
                    [void]$target.AppendChild($children)
                }
                'list' {
                    $list = $xml.CreateElement('one', 'List', $ns)
                    $bullet = $xml.CreateElement('one', 'Bullet', $ns)
                    $bullet.SetAttribute('bullet', '3')
                    $bullet.SetAttribute('fontSize', '11')
                    [void]$list.AppendChild($bullet)
                    [void]$target.PrependChild($list)
                }
                'tag' {
                    if (-not $xml.SelectSingleNode('/one:Page/one:TagDef[@index="0"]', $manager)) {
                        $definition = $xml.CreateDocumentFragment()
                        $definition.InnerXml = '<one:TagDef xmlns:one="' + $ns + '" index="0" type="0" symbol="3" fontColor="automatic" highlightColor="none" name="Native task"/>'
                        [void]$xml.DocumentElement.PrependChild($definition)
                    }
                    $tag = $xml.CreateElement('one', 'Tag', $ns)
                    foreach ($pair in @{ index='0'; completed='false'; disabled='false'; creationDate='2020-01-02T03:04:05.000Z' }.GetEnumerator()) { $tag.SetAttribute($pair.Key, $pair.Value) }
                    [void]$target.PrependChild($tag)
                }
                'format' {
                    $text = $target.SelectSingleNode('one:T', $manager)
                    $plain = if ($case.operation -eq 'join') { 'Right' } else { 'ab' + [char]::ConvertFromUtf32(0x1F980) + 'cd' }
                    $text.InnerText = '<i>' + $plain + '</i>'
                }
                'sibling' { $paragraphs[-1].SelectSingleNode('one:T', $manager).InnerText = 'Native sibling' }
                'adoption' { $left.SelectSingleNode('one:T', $manager).InnerText = 'X' }
                default { throw ('Unknown native control: ' + $case.change) }
            }
            $xml.Save((Join-Path $updates ($case.name + '.xml')))
            Write-Output ('Updating: ' + $case.name)
            $app.UpdatePageContent($xml.OuterXml, [DateTime]::MinValue, 1, $true)
            Write-Output ('Applied: ' + $case.name)
        }
    }
    $app.SyncHierarchy($notebook)
} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
