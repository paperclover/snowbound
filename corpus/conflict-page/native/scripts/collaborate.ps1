param([Parameter(Mandatory=$true)][string]$Root,
      [Parameter(Mandatory=$true)][string]$SharedPath,
      [Parameter(Mandatory=$true)][string]$CloneHost,
      [switch]$ResumeCache,
      [int]$StartSequence = 1)
function Keep-Container($page, $target) {
    $container = $target
    while ($container.ParentNode -ne $page.DocumentElement) { $container = $container.ParentNode }
    foreach ($sibling in @($page.DocumentElement.ChildNodes)) {
        if ($sibling -ne $container -and $sibling.LocalName -in @('Outline', 'Title', 'Image', 'InkDrawing')) {
            [void]$page.DocumentElement.RemoveChild($sibling)
        }
    }
}
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\text.ps1"
if ($SharedPath -notmatch '^\\\\192\.168\.77\.1\\agent\\m6-[a-z0-9-]+$') { throw 'Choose a disposable Linux lab notebook.' }
if ($env:COMPUTERNAME -ne $CloneHost) { throw 'The collaboration controller requires its owned clone.' }
if (-not $ResumeCache) { & "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost }
$shared = $SharedPath
if (-not (Test-Path "$shared\synthetic.one")) { throw 'Create the shared fixture first.' }
New-Item -ItemType Directory "$Root\inbox", "$Root\outbox" -Force | Out-Null
$user = 'HKCU:\Software\Microsoft\Office\Common\UserInfo'
New-Item $user -Force | Out-Null
New-ItemProperty $user -Name UserName -Value $CloneHost -PropertyType String -Force | Out-Null
New-ItemProperty $user -Name UserInitials -Value $CloneHost.Substring($CloneHost.Length - 3) -PropertyType String -Force | Out-Null
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $app.OpenHierarchy($shared, '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    [IO.File]::WriteAllText("$Root\ready", 'ready')
    $deadline = [DateTime]::UtcNow.AddMinutes(30)
    $sequence = $StartSequence
    while ([DateTime]::UtcNow -lt $deadline) {
        $name = '{0:d4}.json' -f $sequence
        $input = "$Root\inbox\$name"
        if (-not (Test-Path $input)) { Start-Sleep -Milliseconds 100; continue }
        $command = Get-Content -Raw -Encoding UTF8 $input | ConvertFrom-Json
        $output = "$Root\outbox\$sequence"
        New-Item -ItemType Directory $output | Out-Null
        try {
            if ($command.action -in @('prepare-stress', 'stress')) {
                . "$PSScriptRoot\stress.ps1"
                Invoke-Stress $app $section $command $output $shared
            }
            elseif ($command.action -eq 'sync') { $app.SyncHierarchy($notebook) }
            elseif ($command.action -eq 'kill-process') {
                Get-Process ONENOTE | Stop-Process -Force
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
                $app = New-Object -ComObject OneNote.Application
                $app.OpenHierarchy($shared, '', [ref]$notebook, 0)
                $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
            }
            elseif ($command.action -eq 'edit' -or $command.action -eq 'snapshot') {
                $hierarchy = ''
                $app.GetHierarchy($section, 4, [ref]$hierarchy, 1)
                [IO.File]::WriteAllText("$output\hierarchy.xml", $hierarchy, [Text.Encoding]::UTF8)
                [xml]$tree = $hierarchy
                $matches = 0
                $editedPage = $null
                $i = 0
                foreach ($node in $tree.SelectNodes('//*[local-name()="Page"]')) {
                    $content = ''
                    $app.GetPageContent($node.GetAttribute('ID'), [ref]$content, 1, 1)
                    [IO.File]::WriteAllText("$output\page-$i.xml", $content, [Text.Encoding]::UTF8)
                    [xml]$page = $content
                    if ($command.action -eq 'edit') {
                        $targets = @($page.SelectNodes('//*[local-name()="T"]') | Where-Object {
                            (Get-ParagraphText $_.InnerText) -eq $command.expected
                        })
                        if ($targets.Count -gt 1) { throw 'Expected one matching text object.' }
                        if ($targets.Count -eq 1) {
                            $matches++
                            if ($matches -gt 1) { throw 'The text occurs on multiple pages.' }
                            $targets[0].InnerText = [Security.SecurityElement]::Escape($command.text)
                            Keep-Container $page $targets[0]
                            $editedPage = $page
                        }
                    }
                    $i++
                }
                if ($command.action -eq 'edit' -and $matches -ne 1) { throw 'Expected one matching text object.' }
                if ($editedPage) { $app.UpdatePageContent($editedPage.OuterXml, [DateTime]::MinValue, 1, $false) }
            } elseif ($command.action -ne 'close') { throw 'Unknown collaboration command.' }
            [IO.File]::WriteAllText("$output\done", 'ok')
        } catch {
            [IO.File]::WriteAllText("$output\error", $_.Exception.ToString())
        }
        if ($command.action -eq 'close') { break }
        $sequence++
    }
} finally {
    try { if ($notebook) { $app.CloseNotebook($notebook, $false) } }
    finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
        $app = $null
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        Get-Process ONENOTE -ErrorAction SilentlyContinue | Wait-Process -Timeout 10
        [IO.File]::WriteAllText("$Root\closed", 'closed')
    }
}
