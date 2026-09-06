param([string]$Root, [string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$results = New-Object Collections.Generic.List[object]
$index = 0
function Record-Phase([string]$Phase) {
    @{ name=$file.BaseName; phase=$Phase; utc=[DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json -Compress | Add-Content "$Root\progress.jsonl" -Encoding UTF8
}
foreach ($file in @(Get-ChildItem "$Root\inputs" -Filter '*.one' | Sort-Object Name)) {
    $output = "$Root\results\$($file.BaseName)"
    New-Item -ItemType Directory $output -Force | Out-Null
    Record-Phase 'cold-reset'
    $cold = $false
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        $case = "C:\one-tests\runs\probe-$index-$attempt"
        New-Item -ItemType Directory "$case\notebook" -Force | Out-Null
        Copy-Item $file.FullName "$case\notebook\synthetic.one"
        Get-Process ONENOTE -ErrorAction SilentlyContinue | ForEach-Object {
            Stop-Process -InputObject $_ -Force -ErrorAction SilentlyContinue
            if (-not $_.WaitForExit(10000)) { throw 'OneNote did not exit before the cache reset.' }
        }
        Start-Sleep -Milliseconds 250
        try { & "$PSScriptRoot\cold-current.ps1" -Root $case -CloneHost $CloneHost }
        catch {
            if ($_.Exception.Message -ne 'Close OneNote before resetting its test cache.') { throw }
            continue
        }
        if (-not (Get-Process ONENOTE -ErrorAction SilentlyContinue)) { $cold = $true; break }
    }
    if (-not $cold) { throw 'OneNote kept reopening during the cache reset.' }
    Record-Phase 'create-application'
    $app = New-Object -ComObject OneNote.Application
    $notebook = ''; $section = ''; $hierarchy = ''; $failure = $null; $pages = @()
    $started = [DateTime]::UtcNow
    try {
        Record-Phase 'open-notebook'
        $app.OpenHierarchy("$case\notebook", '', [ref]$notebook, 0)
        Record-Phase 'open-section'
        $app.OpenHierarchy("$case\notebook\synthetic.one", '', [ref]$section, 0)
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        do {
            Record-Phase 'get-hierarchy'
            $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
            [xml]$tree = $hierarchy
            $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
            if ($pages.Count -and $tree.DocumentElement.GetAttribute('areAllPagesAvailable') -ne 'false') { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $deadline)
        if (-not $pages.Count -or $tree.DocumentElement.GetAttribute('areAllPagesAvailable') -eq 'false') {
            throw 'The section did not finish loading its pages.'
        }
        $n = 0
        foreach ($page in $pages) {
            $content = ''
            Record-Phase "get-page-$n"
            $app.GetPageContent($page.GetAttribute('ID'), [ref]$content, 1, 1)
            [IO.File]::WriteAllText("$output\page-$n.xml", $content, [Text.Encoding]::UTF8)
            $n++
        }
    } catch { $failure = $_ | Out-String }
    finally {
        [IO.File]::WriteAllText("$output\hierarchy.xml", $hierarchy, [Text.Encoding]::UTF8)
        $results.Add(@{ name=$file.BaseName; pages=$pages.Count; error=$failure;
            seconds=([DateTime]::UtcNow - $started).TotalSeconds;
            source_sha256=(Get-FileHash $file.FullName).Hash.ToLowerInvariant() })
        $results | ConvertTo-Json -Depth 5 | Set-Content "$Root\results.json" -Encoding UTF8
        Record-Phase 'close-notebook'
        try { if ($notebook) { $app.CloseNotebook($notebook, $false) } } catch {}
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        Record-Phase 'complete'
    }
    $index++
}
