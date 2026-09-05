function Invoke-Stress($app, $section, $command, $output, $shared) {
    $hierarchy = ''
    $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
    [xml]$tree = $hierarchy
    $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
    if ($pages.Count -ne 1) { throw 'Expected one stress-test page.' }
    $pageId = $pages[0].GetAttribute('ID')
    if ($command.action -eq 'prepare-stress') {
        for ($i = 0; $i -lt $command.clients; $i++) {
            $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $pageId + '"><one:Outline><one:Position x="48" y="' + (150 + 75 * $i) + '" z="' + ($i + 1) + '"/><one:OEChildren><one:OE><one:T><![CDATA[Native ' + $i + ':]]></one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
            $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
        }
        return
    }
    $random = New-Object Random([int]$command.seed)
    $events = New-Object IO.StreamWriter("$output\events.jsonl", $false, [Text.Encoding]::UTF8)
    $events.AutoFlush = $true
    try {
        [IO.File]::WriteAllText("$output\ready", 'ready')
        $deadline = [DateTime]::UtcNow.AddMinutes(10)
        while (-not (Test-Path "$shared\stress-start")) {
            if ([DateTime]::UtcNow -gt $deadline) { throw 'Stress start timed out.' }
            Start-Sleep -Milliseconds 20
        }
        for ($i = 0; $i -lt $command.operations; $i++) {
            Start-Sleep -Milliseconds $random.Next(10, 100)
            $started = [DateTime]::UtcNow.Ticks
            $content = ''
            $app.GetPageContent($pageId, [ref]$content, 1, 1)
            [xml]$page = $content
            $matches = @($page.SelectNodes('//*[local-name()="T"]') | Where-Object {
                (Get-ParagraphText $_.InnerText).StartsWith($command.prefix)
            })
            if ($matches.Count -ne 1) { throw 'Expected one stress-test paragraph.' }
            $before = Get-ParagraphText $matches[0].InnerText
            $token = ' [n' + $command.actor + ':' + $i + ']'
            $start = $end = $before.Length
            $replacement = $token
            $bold = $italic = $false
            if ($command.edit) {
                $boundaries = New-Object 'Collections.Generic.List[int]'
                for ($at = $command.prefix.Length; $at -le $before.Length; $at++) {
                    $boundaries.Add($at)
                    if ($at -lt $before.Length -and [char]::IsHighSurrogate($before[$at])) { $at++ }
                }
                $first = $boundaries[$random.Next($boundaries.Count)]
                $second = $boundaries[$random.Next($boundaries.Count)]
                $start = [Math]::Min($first, $second)
                $end = [Math]::Max($first, $second)
                $replacement = ' caf' + [char]0xe9 + ' ' + [char]::ConvertFromUtf32(0x1f980) + $token
                $bold = $random.Next(2) -eq 1
                $italic = $random.Next(2) -eq 1
            }
            $after = $before.Substring(0, $start) + $replacement + $before.Substring($end)
            $html = [Security.SecurityElement]::Escape($after)
            if ($command.edit) {
                $weight = if ($bold) { 'bold' } else { 'normal' }
                $style = if ($italic) { 'italic' } else { 'normal' }
                $html = '<span style="font-weight:' + $weight + ';font-style:' + $style + '">' + $html + '</span>'
            }
            $matches[0].InnerText = $html
            Keep-Container $page $matches[0]
            $updateStarted = [DateTime]::UtcNow.Ticks
            $app.UpdatePageContent($page.OuterXml, [DateTime]::MinValue, 1, $false)
            $updated = [DateTime]::UtcNow.Ticks
            $events.WriteLine((@{ operation=$i; before=$before; token=$token; started_ticks=$started;
                range=@($start, $end); replacement=$replacement; bold=$bold; italic=$italic;
                update_started_ticks=$updateStarted; updated_ticks=$updated } | ConvertTo-Json -Compress))
            if ($i -eq 0) { [IO.File]::WriteAllText("$output\editing", 'editing') }
            if ($command.sync_every -gt 0 -and (($i + 1) % $command.sync_every) -eq 0) {
                $app.SyncHierarchy($section)
            }
        }
    } finally { $events.Dispose() }
}
