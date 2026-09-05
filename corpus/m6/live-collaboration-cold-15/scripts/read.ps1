param(
    [Parameter(Mandatory=$true)][string]$Root,
    [int]$ExpectedPages = -1,
    [switch]$UseCurrentCache,
    [switch]$Pdf,
    [switch]$KeepOpen,
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
$failure = $null
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
    $deadline = [DateTime]::UtcNow.AddSeconds(300)
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
                $app.GetPageContent($id, [ref]$content, 1, 1)
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
        $index = 0
        foreach ($id in @($pages.Keys | Sort-Object)) {
            [IO.File]::WriteAllText((Join-Path $output ('unsettled-{0:d3}.xml' -f $index)), $pages[$id], [Text.Encoding]::UTF8)
            $index++
        }
        $hierarchy = ''
        $app.GetHierarchy($notebookId, 4, [ref]$hierarchy, 1)
        [IO.File]::WriteAllText((Join-Path $output 'unsettled-hierarchy.xml'), $hierarchy, [Text.Encoding]::UTF8)
        throw "Expected $ExpectedPages pages; OneNote returned $($pages.Count)."
    }
    $index = 0
    $payloads = @()
    foreach ($id in @($pages.Keys | Sort-Object)) {
        [IO.File]::WriteAllText((Join-Path $output ('page-{0:d3}.xml' -f $index)), $pages[$id], [Text.Encoding]::UTF8)
        if ($Pdf) {
            $pdfPath = Join-Path $output ('page-{0:d3}.pdf' -f $index)
            $app.NavigateTo($id, '', $false)
            $app.Publish($id, $pdfPath, 3, '')
            if (-not (Test-Path $pdfPath) -or (Get-Item $pdfPath).Length -eq 0) { throw 'OneNote did not publish the page PDF.' }
        }
        [xml]$page = $pages[$id]
        foreach ($file in $page.SelectNodes('//*[local-name()="InsertedFile" or local-name()="MediaFile"]')) {
            $bytes = [IO.File]::ReadAllBytes($file.GetAttribute('pathCache'))
            $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
            [IO.File]::WriteAllBytes((Join-Path $output ($hash + '.attachment')), $bytes)
            $payloads += @{ page = $id; object = $file.ParentNode.GetAttribute('objectID');
                            kind = $file.LocalName; name = $file.GetAttribute('preferredName');
                            sha256 = $hash; bytes = $bytes.Length }
        }
        $index++
    }
    [IO.File]::WriteAllText((Join-Path $output 'payloads.json'), (ConvertTo-Json -InputObject $payloads -Depth 4), [Text.Encoding]::UTF8)
    $all = ''
    $app.GetHierarchy($notebookId, 4, [ref]$all, 1)
    [xml]$finalTree = $all
    $finalIds = @($finalTree.SelectNodes('//*[local-name()="Page"]') | ForEach-Object { $_.GetAttribute('ID') } | Sort-Object -Unique)
    if ($finalIds.Count -ne $pages.Count -or @($finalIds | Where-Object { -not $pages.ContainsKey($_) }).Count -ne 0) {
        throw 'The notebook hierarchy changed while collecting page evidence; repeat the cold read.'
    }
    [IO.File]::WriteAllText((Join-Path $output 'hierarchy.xml'), $all, [Text.Encoding]::UTF8)
    Write-Output "Read $($sections.Count) sections and $($pages.Count) pages."
} catch {
    $failure = $_
    [IO.File]::WriteAllText((Join-Path $output 'failure.txt'), ($_ | Out-String), [Text.Encoding]::UTF8)
    throw
} finally {
    try {
        try {
            if ($notebookId -and $CloneHost) { $app.SyncHierarchy($notebookId) }
            if ($notebookId -and -not $KeepOpen -and (-not $UseCurrentCache -or $CloneHost)) {
                $app.CloseNotebook($notebookId, $false)
            }
        } catch {
            if ($null -eq $failure) { throw }
            [IO.File]::WriteAllText((Join-Path $output 'cleanup-failure.txt'), ($_ | Out-String), [Text.Encoding]::UTF8)
        }
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
