param([string]$Root, [string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$results = New-Object Collections.Generic.List[object]
$index = 0
foreach ($file in @(Get-ChildItem "$Root\inputs" -Filter '*.one' | Sort-Object Name)) {
    $case = "C:\one-tests\runs\probe-$index"
    $output = "$Root\results\$($file.BaseName)"
    New-Item -ItemType Directory "$case\notebook", $output -Force | Out-Null
    Copy-Item $file.FullName "$case\notebook\synthetic.one"
    Get-Process ONENOTE -ErrorAction SilentlyContinue | ForEach-Object {
        Stop-Process -InputObject $_ -Force
        if (-not $_.WaitForExit(10000)) { throw 'OneNote did not exit before the cache reset.' }
    }
    & "$PSScriptRoot\cold-current.ps1" -Root $case -CloneHost $CloneHost
    $app = New-Object -ComObject OneNote.Application
    $notebook = ''; $section = ''; $hierarchy = ''; $failure = $null; $pages = @()
    $started = [DateTime]::UtcNow
    try {
        $app.OpenHierarchy("$case\notebook", '', [ref]$notebook, 0)
        $app.OpenHierarchy("$case\notebook\synthetic.one", '', [ref]$section, 0)
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        do {
            $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
            [xml]$tree = $hierarchy
            $pages = @($tree.SelectNodes('//*[local-name()="Page"]'))
            if ($pages.Count) { break }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $deadline)
        $n = 0
        foreach ($page in $pages) {
            $content = ''
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
        try { if ($notebook) { $app.CloseNotebook($notebook, $false) } } catch {}
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
    }
    $index++
}
