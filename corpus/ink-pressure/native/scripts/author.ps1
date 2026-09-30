param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
# Pressure ink as OneNote 2010 keeps it: WPF writes strokes with NormalPressure (and one with
# tilt) as ISF, and OneNote takes each through UpdatePageContent into a section of its own.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
Add-Type -AssemblyName PresentationCore, WindowsBase
$P = [System.Windows.Input.StylusPointProperties]
$U = [System.Windows.Input.StylusPointPropertyUnit]

function Info($property, $min, $max, $unit, $resolution) {
    if ($null -eq $min) { return New-Object System.Windows.Input.StylusPointPropertyInfo($property) }
    New-Object System.Windows.Input.StylusPointPropertyInfo($property, $min, $max, $unit, $resolution)
}

# A stroke through `points` of [x, y, pressure 0..1, extra values...] in WPF units (1/96 inch).
function Stroke($points, $width, $height, [switch]$Ignore, [switch]$Rectangle, $infos) {
    if (-not $infos) { $infos = @((Info $P::X), (Info $P::Y), (Info $P::NormalPressure)) }
    $description = New-Object System.Windows.Input.StylusPointDescription(,[System.Windows.Input.StylusPointPropertyInfo[]]$infos)
    $pressure = $infos[2]
    $collection = New-Object System.Windows.Input.StylusPointCollection(,$description)
    foreach ($p in $points) {
        $extra = [int[]]@($p | Select-Object -Skip 3)
        $collection.Add((New-Object System.Windows.Input.StylusPoint($p[0], $p[1], [single]0.5, $description, $extra)))
        $point = $collection[$collection.Count - 1]
        $point.SetPropertyValue([System.Windows.Input.StylusPointProperties]::NormalPressure,
            [int][Math]::Round($pressure.Minimum + $p[2] * ($pressure.Maximum - $pressure.Minimum)))
        $collection[$collection.Count - 1] = $point
    }
    $stroke = New-Object System.Windows.Ink.Stroke -ArgumentList (,$collection)
    $stroke.DrawingAttributes.Width = $width
    $stroke.DrawingAttributes.Height = $height
    $stroke.DrawingAttributes.IgnorePressure = [bool]$Ignore
    $stroke.DrawingAttributes.FitToCurve = $false
    if ($Rectangle) { $stroke.DrawingAttributes.StylusTip = [System.Windows.Ink.StylusTip]::Rectangle }
    $stroke
}

function Isf($strokes) {
    $collection = New-Object System.Windows.Ink.StrokeCollection
    foreach ($stroke in $strokes) { $collection.Add([System.Windows.Ink.Stroke]$stroke) }
    $stream = New-Object IO.MemoryStream
    $collection.Save($stream, $true)
    [Convert]::ToBase64String($stream.ToArray())
}

function Line($x, $y, $pressure, $count, $step) {
    $points = @()
    for ($i = 0; $i -lt $count; $i++) { $points += ,@(($x + $i * $step), $y, $pressure) }
    ,$points
}

# "Levels": one drawing of nine 24-point strokes at pressures 0 to 1 in eighths, and one of
# the same levels on a pen reporting 0..255.
$levels = @(); $coarse = @()
$narrow = @((Info $P::X), (Info $P::Y), (Info $P::NormalPressure 0 255 $U::None 1))
for ($i = 0; $i -le 8; $i++) {
    $levels += Stroke (Line 0 ($i * 48) ($i / 8) 5 20) 32 32
    $coarse += Stroke (Line 0 ($i * 48) ($i / 8) 5 20) 32 32 -infos $narrow
}
# "Strokes": a rising ramp, a wave, a pen ignoring the pressure it recorded, a stroke with
# tilt, and a rectangular tip, each a drawing of its own.
$ramp = @(); $wave = @(); $flat = @(); $tilt = @(); $block = @()
for ($i = 0; $i -le 40; $i++) {
    $ramp += ,@(($i * 8), 0, ($i / 40))
    $wave += ,@(($i * 8), (20 * [Math]::Sin($i / 4)), (0.5 + 0.5 * [Math]::Sin($i / 5)))
    $flat += ,@(($i * 8), 0, ($i / 40))
    $tilt += ,@(($i * 8), 0, (0.2 + $i / 60), (-4500 + $i * 200), 3000)
}
for ($i = 0; $i -le 10; $i++) { $block += ,@(($i * 20), 0, ($i / 10)) }
$tilted = @((Info $P::X), (Info $P::Y), (Info $P::NormalPressure),
    (Info $P::XTiltOrientation -9000 9000 $U::Degrees 100), (Info $P::YTiltOrientation -9000 9000 $U::Degrees 100))
$pages = [ordered]@{
    'Levels' = @(@(36, 90, (Isf $levels)), @(252, 90, (Isf $coarse)))
    'Strokes' = @(
        @(36, 90, (Isf @(Stroke $ramp 8 8))),
        @(36, 150, (Isf @(Stroke $wave 4 4))),
        @(36, 220, (Isf @(Stroke $flat 8 8 -Ignore))),
        @(36, 270, (Isf @(Stroke $tilt 8 8 -infos $tilted))),
        @(36, 320, (Isf @(Stroke $block 4 24 -Rectangle))))
}

$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy((Join-Path $Root 'notebook'), '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('Pressure.one', $notebookId, [ref]$sectionId, 3)
    foreach ($name in $pages.Keys) {
        $pageId = ''
        $app.CreateNewPage($sectionId, [ref]$pageId, 0)
        $drawings = ($pages[$name] | ForEach-Object {
            "<one:InkDrawing><one:Position x=`"$($_[0])`" y=`"$($_[1])`"/><one:Data>$($_[2])</one:Data></one:InkDrawing>"
        }) -join ''
        $xml = "<?xml version=`"1.0`"?><one:Page xmlns:one=`"$namespace`" ID=`"$pageId`"><one:Title><one:OE><one:T><![CDATA[$name]]></one:T></one:OE></one:Title>$drawings</one:Page>"
        [IO.File]::WriteAllText((Join-Path $Root "author-$name.xml"), $xml, [Text.Encoding]::UTF8)
        $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $false)
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
