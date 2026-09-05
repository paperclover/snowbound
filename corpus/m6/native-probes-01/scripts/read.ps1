param(
    [Parameter(Mandatory=$true)][string]$Root,
    [int]$ExpectedPages = -1,
    [switch]$UseCurrentCache,
    [string]$CloneHost = ''
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($Root).TrimEnd('\')
if ([IO.Path]::GetDirectoryName($root) -ne 'C:\one-tests\runs') {
    throw 'Choose a run directly below C:\one-tests\runs.'
}
$notebook = Join-Path $root 'notebook'
$output = Join-Path $root 'read'
if (Test-Path $output) { throw 'Choose a new read destination.' }
if ($UseCurrentCache) {
    if ((Get-ItemProperty 'HKCU:\Software\Microsoft\Office\14.0\OneNote\Options\Paths').UnfiledNotesSection -ne 'C:\one-tests\Loose.one') {
        throw 'Park the personal OneNote profile before reading test notebooks.'
    }
} else {
    & "$PSScriptRoot\cold-current.ps1" -Root $root -CloneHost $CloneHost
}
New-Item -ItemType Directory -Path $output | Out-Null
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy($notebook, '', [ref]$notebookId, 0)
    $process = Get-Process ONENOTE
    @{ hostname = [Environment]::MachineName; onenote = $process.MainModule.FileVersionInfo.FileVersion;
       powershell = $PSVersionTable.PSVersion.ToString(); schema = 'xs2010'; cold = (-not $UseCurrentCache.IsPresent) } |
        ConvertTo-Json | Set-Content (Join-Path $output 'environment.json') -Encoding UTF8
    $sections = @()
    foreach ($file in @(Get-ChildItem $notebook -Recurse | Where-Object { $_.Extension -eq '.one' })) {
        $id = ''
        $app.OpenHierarchy($file.FullName, '', [ref]$id, 0)
        $sections += $id
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $previous = ''
    $stableSince = [DateTime]::UtcNow
    $settled = $false
    do {
        $pages = @{}
        foreach ($section in $sections) {
            $hierarchy = ''
            $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
            [xml]$xml = $hierarchy
            foreach ($node in $xml.SelectNodes('//*[@path]')) {
                if (-not $node.GetAttribute('path').StartsWith("$notebook\", [StringComparison]::OrdinalIgnoreCase)) {
                    throw 'OneNote opened a section outside the copied notebook.'
                }
            }
            foreach ($node in $xml.SelectNodes('//*[local-name()="Page"]')) {
                $id = $node.GetAttribute('ID')
                $content = ''
                $app.GetPageContent($id, [ref]$content, 3, 1)
                $pages[$id] = $content
            }
        }
        $signature = [String]::Join('|', @($pages.Keys | Sort-Object | ForEach-Object { $_ + $pages[$_] }))
        if ($signature -ne $previous) {
            $previous = $signature
            $stableSince = [DateTime]::UtcNow
        }
        if ((($ExpectedPages -ge 0 -and $pages.Count -eq $ExpectedPages) -or
             ($ExpectedPages -lt 0 -and $pages.Count -gt 0)) -and
            ([DateTime]::UtcNow - $stableSince).TotalSeconds -ge 2) { $settled = $true; break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $settled -or ($ExpectedPages -ge 0 -and $pages.Count -ne $ExpectedPages) -or ($ExpectedPages -lt 0 -and $pages.Count -eq 0)) {
        throw "Expected $ExpectedPages pages; OneNote returned $($pages.Count)."
    }
    $index = 0
    foreach ($id in @($pages.Keys | Sort-Object)) {
        [IO.File]::WriteAllText((Join-Path $output ('page-{0:d3}.xml' -f $index)), $pages[$id], [Text.Encoding]::UTF8)
        [xml]$page = $pages[$id]
        foreach ($file in $page.SelectNodes('//*[local-name()="InsertedFile"]')) {
            $bytes = [IO.File]::ReadAllBytes($file.GetAttribute('pathCache'))
            $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
            [IO.File]::WriteAllBytes((Join-Path $output ($hash + '.attachment')), $bytes)
        }
        $index++
    }
    $all = ''
    $app.GetHierarchy($notebookId, 4, [ref]$all, 1)
    [xml]$finalTree = $all
    $finalIds = @($finalTree.SelectNodes('//*[local-name()="Page"]') | ForEach-Object { $_.GetAttribute('ID') } | Sort-Object -Unique)
    if ($finalIds.Count -ne $pages.Count -or @($finalIds | Where-Object { -not $pages.ContainsKey($_) }).Count -ne 0) {
        throw 'The notebook hierarchy changed while collecting page evidence; repeat the cold read.'
    }
    [IO.File]::WriteAllText((Join-Path $output 'hierarchy.xml'), $all, [Text.Encoding]::UTF8)
    Write-Output "Read $($sections.Count) sections and $($pages.Count) pages."
} finally {
    try {
        if ($notebookId -and -not $UseCurrentCache) { $app.CloseNotebook($notebookId, $false) }
    } finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    }
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
    if (-not $UseCurrentCache -and -not $CloneHost) {
        Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 10
    }
}
