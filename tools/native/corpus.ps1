<#
Usage: .\corpus.ps1 -Mode Create -Root C:\one-tests\runs\20260904-01
       .\corpus.ps1 -Mode Capture -Root C:\one-tests\runs\20260904-01 -Step 08-manual-ink
#>
[CmdletBinding()]
param(
    [ValidateSet('Create', 'Capture')]
    [string]$Mode = 'Create',

    [Parameter(Mandatory = $true)]
    [string]$Root,

    [string]$Step
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$schema = 1
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$runs = [IO.Path]::GetFullPath('C:\one-tests\runs').TrimEnd('\')
$root = [IO.Path]::GetFullPath($Root).TrimEnd('\')
$markerPath = Join-Path $root '.native-corpus.json'
$manifestPath = Join-Path $root 'manifest.json'
$logPath = Join-Path $root 'operation-log.jsonl'
$notebookPath = Join-Path $root 'notebook'
$sectionName = 'synthetic.one'
$sectionPath = Join-Path $notebookPath $sectionName
$script:app = $null
$script:openedNotebookId = $null
$script:sectionId = $null
$script:manifest = $null

function Write-Json {
    param(
        $Value,
        [string]$Path
    )

    $Value | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Save-Manifest {
    if ($null -ne $script:manifest) {
        Write-Json $script:manifest $manifestPath
    }
}

function Add-Log {
    param(
        [string]$Action,
        [string]$Fixture,
        $Details
    )

    $entry = [pscustomobject][ordered]@{
        at_utc = [DateTime]::UtcNow.ToString('o')
        fixture = $Fixture
        action = $Action
        details = $Details
    }
    $entry | ConvertTo-Json -Depth 12 -Compress | Add-Content -LiteralPath $logPath -Encoding UTF8
}

function Add-Fixture {
    param(
        [string]$Name,
        [string]$Status,
        [string]$Description,
        $Details
    )

    $script:manifest.fixtures = @($script:manifest.fixtures) + [pscustomobject][ordered]@{
        name = $Name
        status = $Status
        description = $Description
        at_utc = [DateTime]::UtcNow.ToString('o')
        details = $Details
    }
    Save-Manifest
}

function Assert-RunRoot {
    $parent = [IO.Path]::GetDirectoryName($root)
    if (-not [String]::Equals($parent, $runs, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Choose a new direct child of C:\one-tests\runs.'
    }
    if ([String]::Equals($root, $runs, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Choose a run directory below C:\one-tests\runs.'
    }

    $oneTests = [IO.Path]::GetDirectoryName($runs)
    if (-not (Test-Path -LiteralPath $oneTests -PathType Container)) {
        throw 'Create C:\one-tests before running this harness.'
    }
    if (-not (Test-Path -LiteralPath $runs -PathType Container)) {
        New-Item -ItemType Directory -Path $runs | Out-Null
    }
    if (((Get-Item -LiteralPath $runs -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'C:\one-tests\runs cannot be a reparse point.'
    }
}

function Assert-PathUnderRoot {
    param([string]$Path)

    $full = [IO.Path]::GetFullPath($Path)
    if (-not $full.StartsWith("$root\", [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The harness can write only below its run root.'
    }
    return $full
}

function Get-TreeState {
    param([string]$Path)

    $full = Assert-PathUnderRoot $Path
    $files = @(Get-ChildItem -LiteralPath $full -Force -File -Recurse | Sort-Object FullName)
    $entries = @(
        foreach ($file in $files) {
            $relative = $file.FullName.Substring($full.Length).TrimStart('\')
            [pscustomobject][ordered]@{
                path = $relative
                bytes = [int64]$file.Length
                sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    )
    $lines = @($entries | ForEach-Object { "$($_.path)`t$($_.bytes)`t$($_.sha256)" })
    $bytes = [Text.Encoding]::UTF8.GetBytes([String]::Join("`n", $lines))
    $treeHash = [Security.Cryptography.SHA256]::Create().ComputeHash($bytes)
    [pscustomobject][ordered]@{
        file_count = $entries.Count
        byte_count = [int64](($entries | Measure-Object -Property bytes -Sum).Sum)
        sha256 = ([BitConverter]::ToString($treeHash) -replace '-', '').ToLowerInvariant()
        files = $entries
    }
}

function Copy-Snapshot {
    param([string]$Fixture)

    $snapshot = Join-Path (Join-Path $root 'snapshots') $Fixture
    Assert-PathUnderRoot $snapshot | Out-Null
    if (Test-Path -LiteralPath $snapshot) {
        throw "Snapshot '$Fixture' already exists."
    }
    $before = Get-TreeState $notebookPath
    New-Item -ItemType Directory -Path $snapshot | Out-Null
    Copy-Item -LiteralPath $notebookPath -Destination $snapshot -Recurse -Force
    $copied = Get-TreeState (Join-Path $snapshot 'notebook')
    $after = Get-TreeState $notebookPath
    if ($before.sha256 -ne $copied.sha256 -or $before.sha256 -ne $after.sha256) {
        throw "Snapshot '$Fixture' changed while copying."
    }
    $state = [pscustomobject][ordered]@{
        source_before = [pscustomobject][ordered]@{ file_count = $before.file_count; byte_count = $before.byte_count; sha256 = $before.sha256 }
        copied = [pscustomobject][ordered]@{ file_count = $copied.file_count; byte_count = $copied.byte_count; sha256 = $copied.sha256 }
        source_after = [pscustomobject][ordered]@{ file_count = $after.file_count; byte_count = $after.byte_count; sha256 = $after.sha256 }
        files = $copied.files
    }
    Write-Json $state (Join-Path $snapshot 'snapshot.json')
    return $state
}

function Assert-NotebookPaths {
    param([string]$Hierarchy)

    [xml]$document = $Hierarchy
    foreach ($node in @($document.SelectNodes('//*[@path]'))) {
        $path = $node.GetAttribute('path')
        if (-not [String]::IsNullOrWhiteSpace($path)) {
            $full = [IO.Path]::GetFullPath($path)
            if (-not $full.StartsWith("$notebookPath\", [StringComparison]::OrdinalIgnoreCase) -and
                -not [String]::Equals($full.TrimEnd('\'), $notebookPath, [StringComparison]::OrdinalIgnoreCase)) {
                throw 'OneNote returned a hierarchy path outside the generated notebook.'
            }
        }
    }
    return $document
}

function Get-PageXml {
    param([string]$PageId)

    $page = ''
    $script:app.GetPageContent($PageId, [ref]$page, 3, $schema)
    return $page
}

function Get-PageIds {
    param([xml]$Hierarchy)

    $manager = New-Object Xml.XmlNamespaceManager($Hierarchy.NameTable)
    $manager.AddNamespace('one', $namespace)
    return @($Hierarchy.SelectNodes('//one:Page', $manager) | ForEach-Object { $_.GetAttribute('ID') })
}

function Read-ReadyGeneratedNotebook {
    param([int]$ExpectedPageCount = -1)

    if ([String]::IsNullOrWhiteSpace($script:openedNotebookId) -or [String]::IsNullOrWhiteSpace($script:sectionId)) {
        throw 'The generated notebook and section must be open before readiness is checked.'
    }
    $deadline = (Get-Date).AddSeconds(15)
    $lastError = $null
    while ((Get-Date) -lt $deadline) {
        try {
            $hierarchy = ''
            $script:app.GetHierarchy($script:openedNotebookId, 4, [ref]$hierarchy, $schema)
            $document = Assert-NotebookPaths $hierarchy
            $manager = New-Object Xml.XmlNamespaceManager($document.NameTable)
            $manager.AddNamespace('one', $namespace)
            $sections = @($document.SelectNodes('//one:Section', $manager) | Where-Object {
                [String]::Equals($_.GetAttribute('ID'), $script:sectionId, [StringComparison]::OrdinalIgnoreCase) -and
                [String]::Equals([IO.Path]::GetFullPath($_.GetAttribute('path')).TrimEnd('\'), $sectionPath, [StringComparison]::OrdinalIgnoreCase)
            })
            if ($sections.Count -ne 1) { throw 'OneNote has not returned the generated section in the hierarchy.' }
            $pageIds = @(Get-PageIds $document | Where-Object { -not [String]::IsNullOrWhiteSpace($_) })
            if ($ExpectedPageCount -ge 0 -and $pageIds.Count -ne $ExpectedPageCount) {
                throw "Expected $ExpectedPageCount persisted pages, found $($pageIds.Count)."
            }
            $pageContents = @()
            foreach ($pageId in $pageIds) {
                $pageContents += Get-PageXml $pageId
            }
            return [pscustomobject][ordered]@{
                hierarchy = $hierarchy
                page_ids = $pageIds
                page_contents = $pageContents
            }
        }
        catch {
            $lastError = $_.Exception.Message
            Start-Sleep -Milliseconds 250
        }
    }
    throw "OneNote did not return the generated section ready for capture: $lastError"
}

function Get-SolePageId {
    $state = Read-ReadyGeneratedNotebook
    $pages = @($state.page_ids)
    if ($pages.Count -ne 1 -or [String]::IsNullOrWhiteSpace($pages[0])) {
        throw 'The generated notebook must contain exactly one page.'
    }
    return $pages[0]
}

function Confirm-Closed {
    param([string]$NotebookId)

    $probe = ''
    $closed = $false
    try {
        $script:app.GetHierarchy($NotebookId, 0, [ref]$probe, $schema)
        $closed = [String]::IsNullOrWhiteSpace($probe)
    }
    catch {
        $closed = $true
    }
    if (-not $closed) {
        throw 'OneNote still reports the generated notebook as open.'
    }
}

function Write-PageEvidence {
    param(
        [string]$Directory,
        [string]$Hierarchy,
        [string[]]$PageIds,
        [string[]]$PageContents
    )

    if ($PageIds.Count -ne $PageContents.Count) {
        throw 'Page evidence is incomplete.'
    }
    Assert-PathUnderRoot $Directory | Out-Null
    $pagesDirectory = Join-Path $Directory 'pages'
    New-Item -ItemType Directory -Path $pagesDirectory | Out-Null
    [IO.File]::WriteAllText((Join-Path $Directory 'hierarchy.xml'), $Hierarchy, [Text.Encoding]::UTF8)
    $pages = @()
    for ($index = 0; $index -lt $PageIds.Count; $index++) {
        $pageFile = 'page-{0:d2}.xml' -f $index
        [IO.File]::WriteAllText((Join-Path $pagesDirectory $pageFile), $PageContents[$index], [Text.Encoding]::UTF8)
        $pages += [pscustomobject][ordered]@{ id = $PageIds[$index]; file = "pages\\$pageFile" }
    }
    return $pages
}

function Open-GeneratedNotebook {
    param([int]$ExpectedPageCount = -1)

    $opened = ''
    $script:app.OpenHierarchy($notebookPath, '', [ref]$opened, 0)
    if ([String]::IsNullOrWhiteSpace($opened)) {
        throw 'OneNote did not open the generated notebook.'
    }
    $script:openedNotebookId = $opened
    $section = ''
    $script:app.OpenHierarchy($sectionName, $opened, [ref]$section, 0)
    if ([String]::IsNullOrWhiteSpace($section)) {
        throw 'OneNote did not find the generated section.'
    }
    $script:sectionId = $section
    $state = Read-ReadyGeneratedNotebook $ExpectedPageCount
    Add-Log 'open_notebook' $null @{ notebook_id = $opened; path = $notebookPath; expected_page_count = if ($ExpectedPageCount -ge 0) { $ExpectedPageCount } else { $null }; actual_page_count = $state.page_ids.Count }
    return $state
}

function Sync-CloseGeneratedNotebook {
    param(
        [string]$Fixture,
        [string]$Phase
    )

    $closingId = $script:openedNotebookId
    if ([String]::IsNullOrWhiteSpace($closingId)) { throw 'The generated notebook must be open before it is synced.' }
    $script:app.SyncHierarchy($closingId)
    Add-Log 'sync_hierarchy' $Fixture @{ notebook_id = $closingId; phase = $Phase }
    $script:app.CloseNotebook($closingId, $false)
    $script:openedNotebookId = $null
    Confirm-Closed $closingId
    Add-Log 'close_notebook' $Fixture @{ notebook_id = $closingId; phase = $Phase; confirmed = $true }
}

function Capture-Notebook {
    param([string]$Fixture)

    $evidence = Join-Path (Join-Path $root 'evidence') $Fixture
    Assert-PathUnderRoot $evidence | Out-Null
    if (Test-Path -LiteralPath $evidence) {
        throw "Evidence '$Fixture' already exists."
    }
    $baseline = Read-ReadyGeneratedNotebook
    $expectedPageCount = $baseline.page_ids.Count
    Sync-CloseGeneratedNotebook $Fixture 'stabilize'
    $snapshot = Copy-Snapshot $Fixture
    Add-Log 'copy_snapshot' $Fixture @{ sha256 = $snapshot.copied.sha256; file_count = $snapshot.copied.file_count; byte_count = $snapshot.copied.byte_count }
    $state = Open-GeneratedNotebook $expectedPageCount

    New-Item -ItemType Directory -Path $evidence | Out-Null
    $pages = @(Write-PageEvidence $evidence $state.hierarchy $state.page_ids $state.page_contents)
    if ($Fixture -eq '06-attachment') {
        $cachePaths = @(
            foreach ($pageXml in @($state.page_contents)) {
                [xml]$pageDocument = $pageXml
                $manager = New-Object Xml.XmlNamespaceManager($pageDocument.NameTable)
                $manager.AddNamespace('one', $namespace)
                foreach ($file in @($pageDocument.SelectNodes('//one:InsertedFile', $manager))) {
                    if ($file.GetAttribute('preferredName') -eq 'fictitious-attachment.txt' -and -not [String]::IsNullOrWhiteSpace($file.GetAttribute('pathCache'))) {
                        $file.GetAttribute('pathCache')
                    }
                }
            }
        )
        if ($cachePaths.Count -ne 1 -or -not (Test-Path -LiteralPath $cachePaths[0] -PathType Leaf)) {
            throw 'OneNote did not return one readable cached generated attachment.'
        }
        $attachment = [IO.File]::ReadAllBytes($cachePaths[0])
        [IO.File]::WriteAllBytes((Join-Path $evidence 'attachment.txt'), $attachment)
        Add-Log 'capture_attachment_cache' $Fixture @{ bytes = $attachment.Length }
    }
    $native = [pscustomobject][ordered]@{
        fixture = $Fixture
        notebook_id = $script:openedNotebookId
        section_id = $script:sectionId
        hierarchy_file = 'hierarchy.xml'
        pages = $pages
    }
    Write-Json $native (Join-Path $evidence 'native.json')
    Add-Log 'capture_native_api' $Fixture @{ page_count = $pages.Count; schema = $schema }

    Sync-CloseGeneratedNotebook $Fixture 'persist'
    $reopened = Open-GeneratedNotebook $expectedPageCount
    $reopenedDirectory = Join-Path $evidence 'reopened'
    if (Test-Path -LiteralPath $reopenedDirectory) { throw "Reopened evidence for '$Fixture' already exists." }
    New-Item -ItemType Directory -Path $reopenedDirectory | Out-Null
    $reopenedPages = @(Write-PageEvidence $reopenedDirectory $reopened.hierarchy $reopened.page_ids $reopened.page_contents)
    Write-Json ([pscustomobject][ordered]@{
        expected_page_count = $expectedPageCount
        actual_page_count = $reopenedPages.Count
        hierarchy_file = 'hierarchy.xml'
        pages = $reopenedPages
    }) (Join-Path $reopenedDirectory 'native.json')
    Add-Log 'reopen_ready' $Fixture @{ expected_page_count = $expectedPageCount; actual_page_count = $reopenedPages.Count }
    return [pscustomobject][ordered]@{
        evidence = $evidence.Substring($root.Length).TrimStart('\')
        reopened_evidence = (Join-Path (Join-Path 'evidence' $Fixture) 'reopened')
        snapshot = (Join-Path 'snapshots' $Fixture)
    }
}

function Invoke-Step {
    param(
        [string]$Name,
        [string]$Description,
        [scriptblock]$Action
    )

    try {
        Add-Log 'begin_fixture' $Name @{ description = $Description }
        & $Action
        $capture = Capture-Notebook $Name
        Add-Fixture $Name 'captured' $Description $capture
    }
    catch {
        Add-Log 'fixture_failed' $Name @{ error = $_.Exception.Message }
        Add-Fixture $Name 'failed' $Description @{ error = $_.Exception.Message }
        throw
    }
}

function Invoke-PageUpdate {
    param(
        [string]$Fixture,
        [string]$Xml,
        $Details
    )

    $input = Join-Path (Join-Path $root 'inputs') "$Fixture.xml"
    Assert-PathUnderRoot $input | Out-Null
    [IO.File]::WriteAllText($input, $Xml, [Text.Encoding]::UTF8)
    $script:app.UpdatePageContent($Xml, [DateTime]::MinValue, $schema, $false)
    Add-Log 'update_page_content' $Fixture $Details
}

function Get-OfficeMetadata {
    $clsid = (Get-Item -LiteralPath 'Registry::HKEY_CLASSES_ROOT\OneNote.Application\CLSID').GetValue('')
    $server = (Get-Item -LiteralPath "Registry::HKEY_CLASSES_ROOT\CLSID\$clsid\LocalServer32").GetValue('')
    $match = [regex]::Match($server, '^"(?<path>[^"]+\.exe)"|^(?<path>[^\s]+\.exe)', [Text.RegularExpressions.RegexOptions]::IgnoreCase)
    if (-not $match.Success) {
        throw 'OneNote LocalServer32 does not resolve an executable.'
    }
    $path = $match.Groups['path'].Value
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw 'OneNote LocalServer32 executable is unavailable.'
    }
    $version = (Get-Item -LiteralPath $path).VersionInfo.FileVersion
    if ([String]::IsNullOrWhiteSpace($version)) {
        throw 'OneNote executable has no file version.'
    }
    return [pscustomobject][ordered]@{
        prog_id = 'OneNote.Application'
        clsid = $clsid
        local_server = $server
        executable = $path
        file_version = $version
        xml_schema = 'xs2010'
        xml_schema_value = $schema
        xml_namespace = $namespace
    }
}

function Wait-ForStableFile {
    param([string]$Path)

    $full = Assert-PathUnderRoot $Path
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $full -PathType Leaf) {
            $first = Get-Item -LiteralPath $full
            $firstHash = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant()
            Start-Sleep -Milliseconds 500
            $second = Get-Item -LiteralPath $full
            $secondHash = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($first.Length -eq $second.Length -and $firstHash -eq $secondHash) {
                return [pscustomobject][ordered]@{ bytes = [int64]$second.Length; sha256 = $secondHash }
            }
        }
        Start-Sleep -Milliseconds 500
    }
    throw 'OneNote did not finish writing the package within 30 seconds.'
}

$profile = 'HKCU:\Software\Microsoft\Office\14.0\OneNote'
if ((Get-ItemProperty "$profile\Options\Paths").UnfiledNotesSection -ne 'C:\one-tests\Loose.one' -or
    -not (Test-Path 'C:\one-tests\profile-original-cache')) {
    throw 'Park the personal OneNote profile before running native tests.'
}
if (Get-Process ONENOTE -ErrorAction SilentlyContinue) { throw 'Close OneNote before creating fixtures.' }
Assert-RunRoot

if ($Mode -eq 'Create') {
    if (Test-Path -LiteralPath $root) {
        throw 'Choose a root that does not exist.'
    }
    New-Item -ItemType Directory -Path $root | Out-Null
    foreach ($directory in 'assets', 'evidence', 'inputs', 'snapshots') {
        New-Item -ItemType Directory -Path (Join-Path $root $directory) | Out-Null
    }
    Write-Json ([pscustomobject][ordered]@{
        format = 'onenote-native-stage1-root-v1'
        root = $root
        generated = $true
    }) $markerPath
    $script:manifest = [pscustomobject][ordered]@{
        format = 'onenote-native-stage1-v1'
        status = 'running'
        root = $root
        created_at_utc = [DateTime]::UtcNow.ToString('o')
        office = Get-OfficeMetadata
        harness = [pscustomobject][ordered]@{
            source_path = $PSCommandPath
            source_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        notebook = [pscustomobject][ordered]@{ path = 'notebook'; section = $sectionName }
        fixtures = @()
        operation_log = 'operation-log.jsonl'
    }
    Save-Manifest
}
else {
    if (-not (Test-Path -LiteralPath $root -PathType Container) -or -not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw 'Capture requires a generated corpus root.'
    }
    if (((Get-Item -LiteralPath $root -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Capture root cannot be a reparse point.'
    }
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or -not (Test-Path -LiteralPath $notebookPath -PathType Container)) {
        throw 'Capture requires the generated manifest and notebook.'
    }
    try {
        $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    }
    catch {
        throw 'Capture requires a readable generated corpus marker.'
    }
    if ($marker.format -ne 'onenote-native-stage1-root-v1' -or $marker.generated -ne $true -or
        -not [String]::Equals([IO.Path]::GetFullPath($marker.root).TrimEnd('\'), $root, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Capture root marker does not match this generated corpus.'
    }
    if ([String]::IsNullOrWhiteSpace($Step) -or $Step -notmatch '^[0-9]{2}-[a-z0-9-]+$') {
        throw 'Choose a lowercase numbered fixture name for -Step.'
    }
    $script:manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
}

try {
    $script:app = New-Object -ComObject OneNote.Application
    Add-Log 'connect_com' $null @{ prog_id = 'OneNote.Application'; schema = $schema }

    if ($Mode -eq 'Capture') {
        Open-GeneratedNotebook
        $capture = Capture-Notebook $Step
        Add-Fixture $Step 'captured' 'Manual native fixture.' $capture
    }
    else {
        $attachmentPath = Join-Path $root 'assets\fictitious-attachment.txt'
        $imagePath = Join-Path $root 'assets\fictitious-image.png'
        [IO.File]::WriteAllText($attachmentPath, 'Fictitious attachment for native corpus.', [Text.Encoding]::UTF8)
        [IO.File]::WriteAllBytes($imagePath, [Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9J1uoAAAAASUVORK5CYII='))
        Add-Log 'generate_assets' $null @{ attachment = 'assets\fictitious-attachment.txt'; image = 'assets\fictitious-image.png' }

        Invoke-Step '00-empty' 'New notebook and section.' {
            $notebook = ''
            $script:app.OpenHierarchy($notebookPath, '', [ref]$notebook, 1)
            if ([String]::IsNullOrWhiteSpace($notebook)) { throw 'OneNote did not create the generated notebook.' }
            $script:openedNotebookId = $notebook
            $section = ''
            $script:app.OpenHierarchy($sectionName, $notebook, [ref]$section, 3)
            if ([String]::IsNullOrWhiteSpace($section)) { throw 'OneNote did not create the generated section.' }
            $script:sectionId = $section
            Add-Log 'create_notebook_section' '00-empty' @{ notebook_id = $notebook; section_id = $section; path = $notebookPath }
        }

        Invoke-Step '01-blank-page' 'Blank page with no title.' {
            $page = ''
            $script:app.CreateNewPage($script:sectionId, [ref]$page, 2)
            if ([String]::IsNullOrWhiteSpace($page)) { throw 'OneNote did not create the blank page.' }
            Add-Log 'create_new_page' '01-blank-page' @{ page_id = $page; style = 'npsBlankPageNoTitle'; value = 2 }
        }

        Invoke-Step '02-text' 'Plain text outline.' {
            $pageId = Get-SolePageId
            $body = '<one:Outline><one:OEChildren><one:OE><one:T><![CDATA[Fictitious plain text.]]></one:T></one:OE></one:OEChildren></one:Outline>'
            $xml = "<one:Page xmlns:one=`"$namespace`" ID=`"$pageId`">$body</one:Page>"
            Invoke-PageUpdate '02-text' $xml @{ page_id = $pageId; kind = 'plain_text_outline'; input = 'inputs\\02-text.xml' }
        }

        Invoke-Step '03-format-unicode' 'Formatted Unicode text replacing the fetched outline.' {
            $pageId = Get-SolePageId
            $before = Get-PageXml $pageId
            [IO.File]::WriteAllText((Join-Path $root 'evidence\03-format-unicode-before.xml'), $before, [Text.Encoding]::UTF8)
            [xml]$source = $before
            $manager = New-Object Xml.XmlNamespaceManager($source.NameTable)
            $manager.AddNamespace('one', $namespace)
            $outline = $source.SelectSingleNode('//one:Outline[@objectID]', $manager)
            if ($null -eq $outline) { throw 'OneNote did not assign an objectID to the text outline.' }
            $text = $outline.SelectSingleNode('.//one:T', $manager)
            if ($null -eq $text) { throw 'The fetched text outline has no text node.' }
            foreach ($child in @($text.ChildNodes)) { [void]$text.RemoveChild($child) }
            $unicode = 'Fictitious: caf' + [char]0x00E9 + ', ' + [char]0x6771 + [char]0x4EAC + ', ' + [char]0x0645 + [char]0x0631 + [char]0x062D + [char]0x0628 + [char]0x0627
            $html = '<span style="font-family:Calibri;font-size:11.0pt;font-weight:bold;color:#1F4E79">' + $unicode + '</span>'
            [void]$text.AppendChild($source.CreateCDataSection($html))
            $change = New-Object Xml.XmlDocument
            $page = $change.CreateElement('one', 'Page', $namespace)
            $page.SetAttribute('ID', $pageId)
            [void]$page.AppendChild($change.ImportNode($outline, $true))
            [void]$change.AppendChild($page)
            Invoke-PageUpdate '03-format-unicode' $change.OuterXml @{ page_id = $pageId; object_id = $outline.GetAttribute('objectID'); kind = 'full_outline_replace'; input = 'inputs\\03-format-unicode.xml' }
        }

        Invoke-Step '04-positioned-outline' 'Positioned text outline.' {
            $pageId = Get-SolePageId
            $body = '<one:Outline><one:Position x="144" y="96"/><one:Size width="260" height="72"/><one:OEChildren><one:OE><one:T><![CDATA[Fictitious positioned outline.]]></one:T></one:OE></one:OEChildren></one:Outline>'
            $xml = "<one:Page xmlns:one=`"$namespace`" ID=`"$pageId`">$body</one:Page>"
            Invoke-PageUpdate '04-positioned-outline' $xml @{ page_id = $pageId; x = 144; y = 96; input = 'inputs\\04-positioned-outline.xml' }
        }

        Invoke-Step '05-image' 'Generated PNG in a positioned outline.' {
            $pageId = Get-SolePageId
            $data = [Convert]::ToBase64String([IO.File]::ReadAllBytes($imagePath))
            $body = '<one:Outline><one:Position x="144" y="192"/><one:Size width="96" height="96"/><one:OEChildren><one:OE><one:Image format="png"><one:Data>' + $data + '</one:Data></one:Image></one:OE></one:OEChildren></one:Outline>'
            $xml = "<one:Page xmlns:one=`"$namespace`" ID=`"$pageId`">$body</one:Page>"
            Invoke-PageUpdate '05-image' $xml @{ page_id = $pageId; asset = 'assets\fictitious-image.png'; x = 144; y = 192; input = 'inputs\\05-image.xml' }
        }

        Invoke-Step '06-attachment' 'Generated attachment in a positioned outline.' {
            $pageId = Get-SolePageId
            $body = '<one:Outline><one:Position x="264" y="192"/><one:OEChildren><one:OE><one:InsertedFile pathSource="' + [Security.SecurityElement]::Escape($attachmentPath) + '" preferredName="fictitious-attachment.txt"/></one:OE></one:OEChildren></one:Outline>'
            $xml = "<one:Page xmlns:one=`"$namespace`" ID=`"$pageId`">$body</one:Page>"
            Invoke-PageUpdate '06-attachment' $xml @{ page_id = $pageId; asset = 'assets\fictitious-attachment.txt'; x = 264; y = 192; input = 'inputs\\06-attachment.xml' }
        }

        Invoke-Step '07-table' 'Two-cell native table.' {
            $pageId = Get-SolePageId
            $body = '<one:Outline><one:Position x="144" y="312"/><one:OEChildren><one:OE><one:Table bordersVisible="true"><one:Columns><one:Column index="0" width="37.11"/><one:Column index="1" width="37.11"/></one:Columns><one:Row><one:Cell><one:OEChildren><one:OE><one:T><![CDATA[Left cell]]></one:T></one:OE></one:OEChildren></one:Cell><one:Cell><one:OEChildren><one:OE><one:T><![CDATA[Right cell]]></one:T></one:OE></one:OEChildren></one:Cell></one:Row></one:Table></one:OE></one:OEChildren></one:Outline>'
            $xml = "<one:Page xmlns:one=`"$namespace`" ID=`"$pageId`">$body</one:Page>"
            Invoke-PageUpdate '07-table' $xml @{ page_id = $pageId; x = 144; y = 312; input = 'inputs\\07-table.xml'; kind = 'two_cell_table' }
        }

        try {
            $packagePath = Join-Path $root 'notebook.onepkg'
            $script:app.Publish($script:openedNotebookId, $packagePath, 1, '')
            $package = Wait-ForStableFile $packagePath
            $capture = Capture-Notebook '08-onepkg'
            Add-Fixture '08-onepkg' 'captured' 'Published OneNote package.' @{ package = 'notebook.onepkg'; bytes = $package.bytes; sha256 = $package.sha256; capture = $capture }
            Add-Log 'publish_package' '08-onepkg' @{ package = 'notebook.onepkg'; bytes = $package.bytes; sha256 = $package.sha256 }
        }
        catch {
            Add-Log 'fixture_failed' '08-onepkg' @{ error = $_.Exception.Message }
            Add-Fixture '08-onepkg' 'failed' 'Published OneNote package.' @{ error = $_.Exception.Message }
        }
    }

    $script:manifest.status = if (@($script:manifest.fixtures | Where-Object { $_.status -eq 'failed' }).Count -gt 0) { 'completed-with-failures' } else { 'completed' }
    $script:manifest | Add-Member -NotePropertyName completed_at_utc -NotePropertyValue ([DateTime]::UtcNow.ToString('o')) -Force
    Save-Manifest
}
catch {
    if ($null -ne $script:manifest) {
        $script:manifest.status = 'failed'
        $script:manifest | Add-Member -NotePropertyName failure -NotePropertyValue $_.Exception.Message -Force
        Save-Manifest
    }
    throw
}
finally {
    if ($null -ne $script:app -and -not [String]::IsNullOrWhiteSpace($script:openedNotebookId)) {
        try {
            $script:app.CloseNotebook($script:openedNotebookId, $false)
        }
        catch {
        }
    }
}
