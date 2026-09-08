param([Parameter(Mandatory=$true)][string]$Root, [string]$CloneHost = '', [int[]]$Lengths = @(0, 1024))
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$app = New-Object -ComObject OneNote.Application
$notebook = ''
try {
    $inputs = Join-Path $Root 'inputs'
    New-Item -ItemType Directory -Path $inputs | Out-Null
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebook, 0)
    $section = ''
    $app.OpenHierarchy('synthetic.one', $notebook, [ref]$section, 0)
    foreach ($length in $Lengths) {
        $path = Join-Path $inputs ($length.ToString() + '.bin')
        $stream = [IO.File]::Create($path)
        try {
            $block = New-Object byte[] 1024
            for ($i = 0; $i -lt $block.Length; $i++) { $block[$i] = [byte]($i % 251) }
            for ($written = 0; $written -lt $length; $written += $block.Length) { $stream.Write($block, 0, [Math]::Min($block.Length, $length - $written)) }
        } finally { $stream.Dispose() }
        $page = ''
        $app.CreateNewPage($section, [ref]$page, 0)
        $xml = '<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote" ID="' + $page + '"><one:Title><one:OE><one:T>Attachment ' + $length + '</one:T></one:OE></one:Title><one:Outline><one:OEChildren><one:OE><one:InsertedFile pathSource="' + $path + '" preferredName="' + $length + '.bin"/></one:OE></one:OEChildren></one:Outline></one:Page>'
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
    }
    $app.SyncHierarchy($notebook)

} finally {
    if ($notebook) { $app.CloseNotebook($notebook, $false) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
