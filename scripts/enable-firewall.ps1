# One-time, run as Administrator: opens Windows Defender Firewall for Nearby Print.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\enable-firewall.ps1 -ExePath "C:\Path\To\nearby-print.exe"
#
# The app does exactly this on startup via `netsh advfirewall`; use this script
# when it could not (the app logs "firewall: rules were not installed" if it was
# not running with administrator rights).
#
#   TCP 8631 -> IPP printer endpoint
#   UDP 5353 -> mDNS discovery of _ipp._tcp.local.

#Requires -RunAsAdministrator

param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath
)

if (-not (Test-Path -LiteralPath $ExePath)) {
    Write-Error "Executable not found: $ExePath"
    exit 1
}

$rules = @(
    @{ Name = "NearbyPrint - allow TCP 8631"; Protocol = "TCP"; LocalPort = "8631" },
    @{ Name = "NearbyPrint - allow UDP 5353"; Protocol = "UDP"; LocalPort = "5353" }
)

foreach ($rule in $rules) {
    # Remove a stale copy first so repeated runs never pile up duplicates.
    netsh advfirewall firewall delete rule name=$rule.Name | Out-Null

    netsh advfirewall firewall add rule `
        name=$rule.Name `
        dir=in `
        action=allow `
        protocol=$rule.Protocol `
        localport=$rule.LocalPort `
        program="$ExePath" `
        profile=any `
        enable=yes

    if ($LASTEXITCODE -ne 0) {
        Write-Error "Failed to add rule '$($rule.Name)'."
        exit 1
    }

    Write-Host "Allowed $($rule.Protocol) $($rule.LocalPort) for $ExePath" -ForegroundColor Green
}

Write-Host "Done. Nearby Print can now be discovered and reached on the local network."
