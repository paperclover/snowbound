param([Parameter(Mandatory=$true)][string]$LabMac)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$adapter = @(Get-WmiObject Win32_NetworkAdapterConfiguration | Where-Object { $_.MACAddress -eq $LabMac })
if ($adapter.Count -ne 1) { throw 'The clone lab adapter is ambiguous.' }
$enabled = $adapter[0].EnableDHCP()
if ($enabled.ReturnValue -notin @(0, 1)) {
    throw ('Unable to enable DHCP on the clone lab adapter; result ' + $enabled.ReturnValue)
}
$release = $adapter[0].ReleaseDHCPLease()
$result = $adapter[0].RenewDHCPLease()
$deadline = [DateTime]::UtcNow.AddSeconds(60)
do {
    $adapter = @(Get-WmiObject Win32_NetworkAdapterConfiguration | Where-Object { $_.MACAddress -eq $LabMac })
    $addresses = @($adapter[0].IPAddress | Where-Object { $_ -match '^192\.168\.77\.\d+$' -and $_ -ne '192.168.77.1' })
    if ($addresses.Count -eq 1) { break }
    Start-Sleep -Milliseconds 250
} while ([DateTime]::UtcNow -lt $deadline)
if ($addresses.Count -ne 1) { throw ('The clone did not receive a lab IPv4 address; renewal result ' + $result.ReturnValue + '; adapter ' + ($adapter[0] | Select-Object MACAddress,DHCPEnabled,IPEnabled,IPAddress | ConvertTo-Json -Compress)) }
@{mac=$LabMac;address=$addresses[0];releaseResult=$release.ReturnValue;renewalResult=$result.ReturnValue} | ConvertTo-Json -Compress
