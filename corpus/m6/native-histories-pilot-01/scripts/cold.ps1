param([Parameter(Mandatory=$true)][string]$Root, [string]$CloneHost = '')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($Root).TrimEnd('\')
if ([IO.Path]::GetDirectoryName($root) -ne 'C:\one-tests\runs') {
    throw 'Choose a run directly below C:\one-tests\runs.'
}
if (Get-Process ONENOTE -ErrorAction SilentlyContinue) { throw 'Close OneNote before resetting its test cache.' }
$key = 'HKCU:\Software\Microsoft\Office\14.0\OneNote'
if ($CloneHost) {
    if ($CloneHost -notmatch '^ONE-[A-Z0-9-]+$' -or [Environment]::MachineName -ne $CloneHost) {
        throw 'The disposable clone hostname does not match this machine.'
    }
    New-Item "$key\Options\Paths" -Force | Out-Null
    New-ItemProperty "$key\Options\Paths" -Name UnfiledNotesSection -PropertyType ExpandString -Value 'C:\one-tests\Loose.one' -Force | Out-Null
} elseif ((Get-ItemProperty "$key\Options\Paths").UnfiledNotesSection -ne 'C:\one-tests\Loose.one' -or
          -not (Test-Path 'C:\one-tests\profile-original-cache')) {
    throw 'Park the personal OneNote profile before resetting the test cache.'
}
$cache = Join-Path $env:LOCALAPPDATA 'Microsoft\OneNote\14.0'
$parked = Join-Path 'C:\one-tests\caches' ([IO.Path]::GetFileName($root))
if (Test-Path $parked) { throw 'Choose a new run; its parked cache already exists.' }
New-Item -ItemType Directory -Path 'C:\one-tests\caches' -Force | Out-Null
if (Test-Path $cache) { Move-Item -LiteralPath $cache -Destination $parked }
if (Test-Path "$key\OpenNotebooks") { Remove-Item "$key\OpenNotebooks" -Recurse }
New-Item "$key\OpenNotebooks" | Out-Null
New-ItemProperty "$key\OpenNotebooks" -Name '1' -PropertyType String -Value "$root\notebook" | Out-Null
