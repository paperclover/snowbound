param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
# Snowbound makes its folder hidden, which the transfer's archive does not keep.
$sidecar = Get-Item -LiteralPath (Join-Path $Root 'notebook\.snowbound') -Force
$sidecar.Attributes = $sidecar.Attributes -bor [IO.FileAttributes]::Hidden
