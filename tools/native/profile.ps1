# Usage: .\profile.ps1 Isolate|Restore|Status
# Parks the native OneNote profile for tests. It never changes notebook files.
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('Isolate', 'Restore', 'Status')]
    [string]$Command
)

$ErrorActionPreference = 'Stop'
$root = 'C:\one-tests'
$key = 'HKCU:\Software\Microsoft\Office\14.0\OneNote'
$regKey = 'HKCU\Software\Microsoft\Office\14.0\OneNote'
$cache = Join-Path $env:LOCALAPPDATA 'Microsoft\OneNote\14.0'
$originalReg = Join-Path $root 'profile-original.reg'
$originalCache = Join-Path $root 'profile-original-cache'
$testReg = Join-Path $root 'profile-test.reg'
$testCache = Join-Path $root 'profile-test-cache'

function Test-IsolatedProfile {
    if (-not (Test-Path -LiteralPath "$key\Options\Paths")) { return $false }
    try {
        return (Get-ItemProperty -LiteralPath "$key\Options\Paths" -Name UnfiledNotesSection).UnfiledNotesSection -eq "$root\Loose.one"
    } catch {
        return $false
    }
}

if ($Command -ne 'Status' -and (Get-Process ONENOTE -ErrorAction SilentlyContinue)) {
    throw 'Close OneNote, then run this command again.'
}

if ($Command -eq 'Status') {
    $profile = if (Test-IsolatedProfile) { 'test' } elseif (Test-Path -LiteralPath $key) { 'other' } else { 'missing' }
    $activeCache = Test-Path -LiteralPath $cache
    $originals = (Test-Path -LiteralPath $originalReg) -and (Test-Path -LiteralPath $originalCache)
    $tests = (Test-Path -LiteralPath $testReg) -or (Test-Path -LiteralPath $testCache)

    if ($originals -and $profile -eq 'test') {
        Write-Output 'State: isolated. Run Restore when native tests finish.'
    } elseif ($originals -and ($tests -or $profile -eq 'missing')) {
        Write-Output 'State: restore can resume. Run Restore with OneNote closed.'
    } elseif ((Test-Path -LiteralPath $originalReg) -and -not (Test-Path -LiteralPath $originalCache) -and $tests -and $activeCache -and $profile -eq 'other') {
        Write-Output 'State: restored. Test backups remain parked.'
    } else {
        Write-Output 'State: unrecognized. Inspect the paths below before changing the profile.'
    }

    Write-Output "Registry: $profile"
    Write-Output "Active cache: $activeCache"
    Write-Output "Original registry backup: $(Test-Path -LiteralPath $originalReg)"
    Write-Output "Original cache backup: $(Test-Path -LiteralPath $originalCache)"
    Write-Output "Test registry backup: $(Test-Path -LiteralPath $testReg)"
    Write-Output "Test cache backup: $(Test-Path -LiteralPath $testCache)"
    return
}

if ($Command -eq 'Isolate') {
    if ((Test-Path -LiteralPath $originalReg) -or (Test-Path -LiteralPath $originalCache)) {
        throw 'Original backup already exists. Run Status or Restore before isolating again.'
    }
    if ((Test-Path -LiteralPath $testReg) -or (Test-Path -LiteralPath $testCache)) {
        throw 'Test backup already exists. Inspect Status before isolating again.'
    }
    if (-not (Test-Path -LiteralPath $key)) { throw 'The OneNote profile registry was not found. Nothing changed.' }
    if (-not (Test-Path -LiteralPath $cache)) { throw 'The OneNote cache was not found. Nothing changed.' }

    New-Item -ItemType Directory -Path $root -Force | Out-Null
    & reg.exe export $regKey $originalReg
    if ($LASTEXITCODE -ne 0) { throw 'The original registry profile was not saved. Nothing changed.' }
    Move-Item -LiteralPath $cache -Destination $originalCache
    & reg.exe delete $regKey /f
    if ($LASTEXITCODE -ne 0) { throw 'The original registry remains saved. Run Restore to recover it.' }
    New-Item -Path $key -Force | Out-Null
    New-ItemProperty -Path $key -Name FirstBootStatus -PropertyType DWord -Value 0x1000101 | Out-Null
    New-Item -Path "$key\Options\Paths" -Force | Out-Null
    New-ItemProperty -Path "$key\Options\Paths" -Name UnfiledNotesSection -PropertyType ExpandString -Value "$root\Loose.one" | Out-Null
    Write-Output 'State: isolated. The original profile and cache are parked.'
    return
}

if (-not (Test-Path -LiteralPath $originalReg)) { throw 'The original registry backup is missing. Nothing changed.' }
if (-not (Test-Path -LiteralPath $originalCache)) {
    if ((Test-Path -LiteralPath $cache) -and (Test-Path -LiteralPath $testReg) -and -not (Test-IsolatedProfile)) {
        Write-Output 'State: restored. Test backups remain parked.'
        return
    }
    throw 'The original cache backup is missing. Inspect Status before restoring.'
}
if ((Test-Path -LiteralPath $cache) -and (Test-Path -LiteralPath $testCache)) {
    throw 'Both active and parked test caches exist. Inspect Status before restoring.'
}
if (-not (Test-Path -LiteralPath $testReg) -and (Test-Path -LiteralPath $key)) {
    & reg.exe export $regKey $testReg
    if ($LASTEXITCODE -ne 0) { throw 'The test registry profile was not saved. Nothing changed.' }
}
if ((Test-Path -LiteralPath $cache) -and -not (Test-Path -LiteralPath $testCache)) {
    Move-Item -LiteralPath $cache -Destination $testCache
}
if (Test-Path -LiteralPath $key) {
    & reg.exe delete $regKey /f
    if ($LASTEXITCODE -ne 0) { throw 'The test profile is parked. Run Restore to try again.' }
}
& reg.exe import $originalReg
if ($LASTEXITCODE -ne 0) { throw 'The test profile is parked. Run Restore to try again.' }
if (Test-Path -LiteralPath $cache) { throw 'The original cache is still parked. Run Status before restoring.' }
Move-Item -LiteralPath $originalCache -Destination $cache
Write-Output 'State: restored. Test backups remain parked.'
