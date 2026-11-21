# ---------------------------------------------------------------------------
# NearbyPrint fixup, part 3 - remove the port that still names the old address.
#   `NearbyPrint - Nootan` now runs on loopback, so nothing points at
#   http://192.168.1.6:8631/ipp/print any more. Remove-PrinterPort refuses it
#   (it belongs to the Internet Print Provider rather than a port monitor), so
#   the provider's own registry record is removed with the spooler stopped.
#   Nothing uses it - that is checked first, and if anything does, it is kept.
# ---------------------------------------------------------------------------
[CmdletBinding()]
param(
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'
if (-not $LogPath) { $LogPath = Join-Path $env:TEMP 'nearbyprint-fixup3.log' }

function Say($Text) { "$(Get-Date -Format 'HH:mm:ss')  $Text" | Add-Content -LiteralPath $LogPath }
Set-Content -LiteralPath $LogPath -Value ("=== NearbyPrint fixup 3 " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + " ===")

$Identity  = [Security.Principal.WindowsIdentity]::GetCurrent()
$Principal = New-Object Security.Principal.WindowsPrincipal($Identity)
if (-not $Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    try {
        $Escalate = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden',
                      '-File', ('"' + $PSCommandPath + '"'))
        if ($LogPath) { $Escalate += @('-LogPath', ('"' + $LogPath + '"')) }
        $Elevated = Start-Process -FilePath 'powershell.exe' -ArgumentList $Escalate `
                    -Verb RunAs -Wait -PassThru
        exit $Elevated.ExitCode
    } catch {
        Say ('elevation refused: ' + $_.Exception.Message)
        exit 2
    }
}

$Stale = 'http://192.168.1.6:8631/ipp/print'
$Loopback = 'http://127.0.0.1:8631/ipp/print'
$Queue = 'NearbyPrint - Nootan'

# 0. Never remove something a queue still points at.
$Users = @(Get-Printer | Where-Object { $_.PortName -eq $Stale })
if ($Users.Count -gt 0) {
    Say ("--   '{0}' is still used by: {1} - kept" -f $Stale, (($Users.Name) -join ', '))
    exit 0
}
if (-not (Get-PrinterPort -Name $Stale -ErrorAction SilentlyContinue) -and
    -not (Test-Path -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Control\Print\Providers\Internet Print Provider\Ports\$Stale")) {
    Say "--   '$Stale' is already gone"
    exit 0
}

# 1. Make sure the live queue is where it should be before touching anything.
$Live = Get-Printer -Name $Queue -ErrorAction SilentlyContinue
if ($Live) {
    Say ("     '{0}' on port '{1}'" -f $Queue, $Live.PortName)
} else {
    Say "ERR  '$Queue' is missing - aborting rather than changing more state"
    exit 1
}

# 2. Remove the provider record with the spooler stopped.
$Key = "HKLM:\SYSTEM\CurrentControlSet\Control\Print\Providers\Internet Print Provider\Ports\$Stale"
try {
    Stop-Service -Name Spooler -Force -ErrorAction Stop
    Start-Sleep -Seconds 2
    if (Test-Path -LiteralPath $Key) {
        Remove-Item -LiteralPath $Key -Recurse -Force -ErrorAction Stop
        Say 'OK   removed Internet Print Provider record for the old address'
    } else { Say '--   provider record was not present' }
    Start-Service -Name Spooler -ErrorAction Stop
    for ($i = 0; $i -lt 40 -and (Get-Service -Name Spooler).Status -ne 'Running'; $i++) { Start-Sleep -Milliseconds 500 }
} catch {
    Say ('ERR  ' + $_.Exception.Message)
    if ((Get-Service -Name Spooler).Status -ne 'Running') { Start-Service -Name Spooler -ErrorAction SilentlyContinue }
}

# The spooler's own port object may outlive the provider record.
if (Get-PrinterPort -Name $Stale -ErrorAction SilentlyContinue) {
    try {
        Remove-PrinterPort -Name $Stale -ErrorAction Stop
        Say 'OK   removed the spooler port object'
    } catch { Say ('note port object kept: ' + $_.Exception.Message) }
}

# 3. Final state.
Start-Sleep -Seconds 2
Say '--- queues ---'
foreach ($p in Get-Printer) { Say ("   {0,-26} port={1} status={2}" -f $p.Name, $p.PortName, $p.PrinterStatus) }
Say '--- http/WSD ports left ---'
$left = @(Get-PrinterPort | Where-Object { $_.Name -like 'http*' -or $_.Name -like 'WSD*' })
if ($left.Count -eq 0) { Say '   (none)' } else { foreach ($l in $left) { Say ("   {0}" -f $l.Name) } }

$Q = Get-Printer -Name $Queue -ErrorAction SilentlyContinue
if ($Q -and $Q.PortName -like 'http://127.0.0.1*') { Say "OK   '$Queue' is on loopback" }
elseif ($Q) { Say ("note '$Queue' port is '" + $Q.PortName + "' (created by Windows from the loopback URL)") }
else { Say "ERR  '$Queue' missing" }

try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $n = @(Get-CimInstance -ClassName Win32_Printer).Count
    $sw.Stop()
    Say ("Win32_Printer: {0} queues in {1:n2}s" -f $n, $sw.Elapsed.TotalSeconds)
} catch { Say ('ERR  timing: ' + $_.Exception.Message) }

Say '=== done ==='
exit 0
