# ---------------------------------------------------------------------------
# NearbyPrint fixup, part 2
#   1. clear PRINTER_ATTRIBUTE_WORK_OFFLINE (0x400) on `HP LaserJet (Shop)`
#      through the spooler API. Writing the registry while the service runs is
#      overwritten from memory within seconds, and writing it while stopped was
#      also undone - so the flag is changed where Windows keeps the truth.
#   2. drop ports nothing points at any more (the old LAN address, the removed
#      queue's WSD port)
#   3. re-time Win32_Printer, the measurement that found all of this
# ---------------------------------------------------------------------------
[CmdletBinding()]
param(
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'
if (-not $LogPath) { $LogPath = Join-Path $env:TEMP 'nearbyprint-fixup2.log' }

function Say($Text) { "$(Get-Date -Format 'HH:mm:ss')  $Text" | Add-Content -LiteralPath $LogPath }
Set-Content -LiteralPath $LogPath -Value ("=== NearbyPrint fixup 2 " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + " ===")

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

# --- Win32 spooler surface -------------------------------------------------
Add-Type -Namespace NearbyFixup -Name Spooler -MemberDefinition @'
[DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool OpenPrinter(string pPrinterName, out IntPtr phPrinter, IntPtr pDefault);
[DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool ClosePrinter(IntPtr hPrinter);
[DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool GetPrinter(IntPtr hPrinter, uint Level, IntPtr pPrinterInfo, uint cbBuf, out uint pcbNeeded);
[DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool SetPrinter(IntPtr hPrinter, uint Level, IntPtr pPrinterInfo, uint Command);
'@

$Target = 'HP LaserJet (Shop)'
$OfflineBit = 0x400
$RegKey = "HKLM:\SYSTEM\CurrentControlSet\Control\Print\Printers\$Target"

# PRINTER_INFO_5: pPrinterName, pPortName, Attributes, DeviceNotSelectedTimeout,
# WorkOfflineJobTimeout. Attributes is the third field - two pointers in, so
# offset 16 on x64.
$AttrOffset = 16

function Read-Attributes {
    $v = (Get-ItemProperty -LiteralPath $RegKey -Name Attributes -ErrorAction SilentlyContinue).Attributes
    if ($null -eq $v) { return $null }
    return [int]$v
}

# --- 1. clear the flag through the spooler ---------------------------------
$Before = Read-Attributes
Say ("registry before: 0x{0:X}  WORK_OFFLINE={1}" -f $Before, [bool]($Before -band $OfflineBit))

$Handle = [IntPtr]::Zero
$Cleared = $false
try {
    if (-not [NearbyFixup.Spooler]::OpenPrinter($Target, [ref]$Handle, [IntPtr]::Zero)) {
        throw ("OpenPrinter failed, Win32 error " + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
    }
    $Needed = [uint32]0
    [void][NearbyFixup.Spooler]::GetPrinter($Handle, 5, [IntPtr]::Zero, 0, [ref]$Needed)
    if ($Needed -eq 0) { throw 'GetPrinter(5) reported no bytes needed' }

    $Buf = [Runtime.InteropServices.Marshal]::AllocHGlobal([int]$Needed)
    try {
        $Got = [uint32]0
        if (-not [NearbyFixup.Spooler]::GetPrinter($Handle, 5, $Buf, $Needed, [ref]$Got)) {
            throw ("GetPrinter(5) failed, Win32 error " + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
        }
        $AttrField = [IntPtr]::Add($Buf, $AttrOffset)
        $Live = [Runtime.InteropServices.Marshal]::ReadInt32($AttrField)
        Say ("spooler before:  0x{0:X}" -f $Live)
        $New = $Live -band (-bnot $OfflineBit)
        if ($New -eq $Live) {
            Say 'already clear in the spooler'
            $Cleared = $true
        } else {
            [Runtime.InteropServices.Marshal]::WriteInt32($AttrField, $New)
            if (-not [NearbyFixup.Spooler]::SetPrinter($Handle, 5, $Buf, 0)) {
                throw ("SetPrinter(5) failed, Win32 error " + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
            }
            Say ("spooler after:   0x{0:X}  (0x{1:X} -> 0x{2:X})" -f $New, $Live, $New)
            $Cleared = $true
        }
    } finally {
        [Runtime.InteropServices.Marshal]::FreeHGlobal($Buf)
    }
} catch {
    Say ('ERR  spooler API: ' + $_.Exception.Message)
} finally {
    if ($Handle -ne [IntPtr]::Zero) { [void][NearbyFixup.Spooler]::ClosePrinter($Handle) }
}

# Fallback: stop the service, edit the value, start it again. Slower and it
# disturbs printing, so it only runs when the direct path did not work.
if (-not $Cleared) {
    $v = Read-Attributes
    if ($null -ne $v -and ($v -band $OfflineBit)) {
        Say 'fallback: stop spooler -> clear bit -> start spooler'
        try {
            Stop-Service -Name Spooler -Force -ErrorAction Stop
            Start-Sleep -Seconds 2
            $mid = Read-Attributes
            Set-ItemProperty -LiteralPath $RegKey -Name Attributes -Value ($v -band (-bnot $OfflineBit))
            Say ("  wrote 0x{0:X} while stopped (read back 0x{1:X})" -f ($v -band (-bnot $OfflineBit)), (Read-Attributes))
            Start-Service -Name Spooler -ErrorAction Stop
            for ($i = 0; $i -lt 40 -and (Get-Service -Name Spooler).Status -ne 'Running'; $i++) { Start-Sleep -Milliseconds 500 }
            $Cleared = $true
        } catch {
            Say ('ERR  fallback: ' + $_.Exception.Message)
        }
    } else {
        Say 'fallback not needed - registry already clear'
        $Cleared = $true
    }
}

Start-Sleep -Seconds 3
$After = Read-Attributes
Say ("registry after:  0x{0:X}  WORK_OFFLINE={1}" -f $After, [bool]($After -band $OfflineBit))

# --- 2. ports nothing points at --------------------------------------------
$Used = @(Get-Printer | ForEach-Object PortName)
$Orphans = @(
    'http://192.168.1.6:8631/ipp/print',
    'WSD-5ea37ef3-2b30-4980-b27f-a9df11d9fe66'
)
foreach ($o in $Orphans) {
    if ($Used -contains $o) {
        Say "--   '$o' is still used by a queue - kept"
    } elseif (Get-PrinterPort -Name $o -ErrorAction SilentlyContinue) {
        try {
            Remove-PrinterPort -Name $o -ErrorAction Stop
            Say "OK   removed orphan port '$o'"
        } catch { Say ("note port kept: " + $_.Exception.Message) }
    } else { Say "--   '$o' not present" }
}

# --- 3. the measurement that found the stall -------------------------------
try {
    foreach ($run in 1..2) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $rows = @(Get-CimInstance -ClassName Win32_Printer | Select-Object Name, WorkOffline, PrinterStatus)
        $sw.Stop()
        Say ("Win32_Printer run {0}: {1:n2}s ({2} queues)" -f $run, $sw.Elapsed.TotalSeconds, $rows.Count)
        if ($run -eq 1) {
            foreach ($r in $rows) {
                Say ("   {0,-26} WorkOffline={1} status={2}" -f $r.Name, $r.WorkOffline, $r.PrinterStatus)
            }
        }
    }
} catch { Say ('ERR  timing: ' + $_.Exception.Message) }

Say '=== done ==='
exit 0
