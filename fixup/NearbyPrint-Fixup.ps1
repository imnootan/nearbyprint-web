# ---------------------------------------------------------------------------
# NearbyPrint - one-shot Windows fixup
#   1. point `NearbyPrint - Nootan` at 127.0.0.1 (its port still names an old
#      address, and a connect that fails after 38s stalls every printer query)
#   2. remove the foreign `NearbyPrint - Yogeshwar` queue (stuck in Error)
#   3. clear the "Use Printer Offline" flag on `HP LaserJet (Shop)`
#
# Elevated output never reaches the caller, so everything is mirrored to
# $LogPath and read back afterwards.
# ---------------------------------------------------------------------------
[CmdletBinding()]
param(
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'
if (-not $LogPath) { $LogPath = Join-Path $env:TEMP 'nearbyprint-fixup.log' }

function Say($Text) { "$(Get-Date -Format 'HH:mm:ss')  $Text" | Add-Content -LiteralPath $LogPath }
Set-Content -LiteralPath $LogPath -Value ("=== NearbyPrint fixup " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + " ===")

# Add-Printer / Remove-Printer / service control all need administrator, so
# this file relaunches itself once. One UAC prompt, nothing to type.
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

$Driver = 'Microsoft IPP Class Driver'
$Queue  = 'NearbyPrint - Nootan'
$Url    = 'http://127.0.0.1:8631/ipp/print'
$Target = 'HP LaserJet (Shop)'

# 0. The queue only works while this PC's own server is answering. A loopback
#    that nothing listens on fails instantly rather than after 38s, so this is
#    a correctness check, not a stall check.
$Up = $false
try {
    $Client  = New-Object Net.Sockets.TcpClient
    $Pending = $Client.BeginConnect('127.0.0.1', 8631, $null, $null)
    $Up = $Pending.AsyncWaitHandle.WaitOne(3000, $false)
    if ($Up) { $Client.EndConnect($Pending) }
    $Client.Close()
} catch { $Up = $false }
Say ("local print server on 127.0.0.1:8631: " + $(if ($Up) { 'listening' } else { 'NOT LISTENING' }))
if (-not $Up) { exit 1 }

# 1. Foreign queue discovered on the new network, already in Error. Its WSD
#    port is another source of slow enumeration.
$Foreign = 'NearbyPrint - Yogeshwar'
if (Get-Printer -Name $Foreign -ErrorAction SilentlyContinue) {
    try {
        Remove-Printer -Name $Foreign -ErrorAction Stop
        Say "OK   removed queue '$Foreign'"
    } catch {
        Say ('ERR  remove ' + $Foreign + ': ' + $_.Exception.Message)
    }
} else {
    Say "--   '$Foreign' not present"
}

# 2. Repoint the host's own queue at loopback.
#    `Add-Printer -IppURL` is the clean path but has failed on this build before;
#    `printui /if` is Microsoft's silent fallback and is what created this queue.
$Existing = Get-Printer -Name $Queue -ErrorAction SilentlyContinue
if ($Existing -and $Existing.PortName -eq $Url) {
    Say "--   '$Queue' already on $Url"
} else {
    # A queue already removed by an earlier attempt is not an error here - the
    # creation path below is the same either way.
    $OldPort = ''
    $OldDriver = $Driver
    if ($Existing) {
        $OldPort = $Existing.PortName
        $OldDriver = $Existing.DriverName
        Say "     '$Queue' on driver '$OldDriver', port '$OldPort' -> '$Url'"
        try { Remove-Printer -Name $Queue -ErrorAction Stop }
        catch { Say ('ERR  could not remove the queue: ' + $_.Exception.Message); exit 1 }
    } else {
        Say "     '$Queue' is missing - creating it on '$Url' with driver '$Driver'"
    }

    # `IppURL` and `DriverName`/`PortName` belong to *different* parameter sets
    # of Add-Printer, so asking for all three always fails with "Parameter set
    # cannot be resolved" - which is what the first attempt here did before.
    $Added = $false
    try {
        Add-Printer -Name $Queue -IppURL $Url -ErrorAction Stop | Out-Null
        $Added = $true
        Say 'OK   recreated with Add-Printer -IppURL'
    } catch {
        Say ('note Add-Printer -IppURL: ' + $_.Exception.Message)
    }

    if (-not $Added) {
        & rundll32.exe 'printui.dll,PrintUIEntry' /if /b $Queue /r $Url /m $Driver
        for ($i = 0; $i -lt 40 -and -not (Get-Printer -Name $Queue -ErrorAction SilentlyContinue); $i++) {
            Start-Sleep -Milliseconds 500
        }
        if (Get-Printer -Name $Queue -ErrorAction SilentlyContinue) {
            $Added = $true
            Say 'OK   recreated with printui /if'
        } else {
            Say 'note printui /if did not install the queue'
        }
    }

    # Last resort: rebuild on whatever port already exists so the queue is back
    # at all. It keeps the old address, so this reports itself as a partial fix
    # rather than pretending the loopback port was created.
    $Partial = $false
    if (-not $Added) {
        $FallbackPort = $OldPort
        if (-not (Get-PrinterPort -Name $FallbackPort -ErrorAction SilentlyContinue)) {
            $FallbackPort = $Url
        }
        try {
            Add-Printer -Name $Queue -DriverName $Driver -PortName $FallbackPort -ErrorAction Stop | Out-Null
            $Added = $true
            $Partial = ($FallbackPort -ne $Url)
            Say ("OK   recreated on port '" + $FallbackPort + "'" + $(if ($Partial) { ' (STILL THE OLD ADDRESS)' } else { '' }))
        } catch {
            Say ('ERR  Add-Printer -PortName: ' + $_.Exception.Message)
        }
    }

    if (-not $Added) {
        Say "ERR  could not recreate '$Queue' - it is currently MISSING"
        exit 1
    }

    # The old address is now a dangling port; drop it once nothing uses it.
    if ($OldPort -and $OldPort -ne $Url -and
        -not (@(Get-Printer) | Where-Object { $_.PortName -eq $OldPort })) {
        try { Remove-PrinterPort -Name $OldPort -ErrorAction Stop; Say "OK   removed stale port '$OldPort'" }
        catch { Say ('note stale port kept: ' + $_.Exception.Message) }
    }
}

# 3. Clear PRINTER_ATTRIBUTE_WORK_OFFLINE (0x400). Windows keeps this in the
#    spooler's copy of the printer, so the flag is changed while the spooler is
#    stopped and read back fresh on start - writing it while the service runs
#    would simply be overwritten from memory.
$RegKey = "HKLM:\SYSTEM\CurrentControlSet\Control\Print\Printers\$Target"
$Before = (Get-ItemProperty -LiteralPath $RegKey -Name Attributes -ErrorAction SilentlyContinue).Attributes
if ($null -eq $Before) {
    Say "note no Attributes value for '$Target'"
} elseif (($Before -band 0x400) -eq 0) {
    Say ("--   '" + $Target + "' is already online (0x{0:X})" -f $Before)
} else {
    $After = $Before -band (-bnot 0x400)
    try {
        Stop-Service -Name Spooler -Force -ErrorAction Stop
        Set-ItemProperty -LiteralPath $RegKey -Name Attributes -Value $After
        Start-Service -Name Spooler -ErrorAction Stop
        for ($i = 0; $i -lt 40 -and (Get-Service -Name Spooler).Status -ne 'Running'; $i++) {
            Start-Sleep -Milliseconds 500
        }
        Say ("OK   cleared offline flag on '" + $Target + "' (0x{0:X} -> 0x{1:X}); spooler " -f $Before, $After) +
            (Get-Service -Name Spooler).Status
    } catch {
        Say ('ERR  offline flag: ' + $_.Exception.Message)
    }
}

# 4. Verify the way it was diagnosed: enumerate Win32_Printer and time it.
#    Before the fix this took ~51s; a loopback port should bring it back under
#    a second, which is what makes the app's 8s budget meaningful again.
try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $rows = @(Get-CimInstance -ClassName Win32_Printer | Select-Object Name, WorkOffline, PrinterStatus)
    $sw.Stop()
    Say ("     Win32_Printer now enumerates in {0:n2}s ({1} queues)" -f $sw.Elapsed.TotalSeconds, $rows.Count)
    foreach ($r in $rows) {
        Say ("       {0,-26} WorkOffline={1} status={2}" -f $r.Name, $r.WorkOffline, $r.PrinterStatus)
    }
} catch {
    Say ('ERR  verification query: ' + $_.Exception.Message)
}

$Final = Get-Printer -Name $Queue -ErrorAction SilentlyContinue
if ($Final) {
    Say ("final queue: '{0}' on port '{1}' driver '{2}'" -f $Final.Name, $Final.PortName, $Final.DriverName)
} else {
    Say "final queue: '$Queue' MISSING"
}
Say '=== done ==='
exit 0
