#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use axum::{
    body::{Body, Bytes},
    extract::{connect_info::ConnectInfo, DefaultBodyLimit, Request, State},
    http::{HeaderMap, Method, Uri},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use ipp::parser::IppParser;
use ipp::prelude::*;
use ipp::value::IppName;
use local_ip_address::list_afinet_netifas;
use mdns_sd::{DaemonStatus, ServiceDaemon, ServiceEvent, ServiceInfo};
use printers::common::base::printer::PrinterState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use uuid::Uuid;

const SERVICE_NAME_PREFIX: &str = "NearbyPrint";

/// The network-facing IPP server: the one phones, other PCs and every host
/// this app advertises over mDNS answer on.
const IPP_PORT: u16 = 8631;

/// The loopback-only IPP proxy the installed Windows virtual printer posts to.
///
/// Deliberately *not* [`IPP_PORT`]: both listeners live in one process on a
/// client PC, and two binds on the same port fail with `AddrInUse` - reported
/// on Windows as `os error 10048` - the moment the second one starts. Keeping
/// the local proxy on its own port is what makes the two roles coexist: the
/// network server may be off entirely while `Ctrl+P` still works.
const LOCAL_PROXY_PORT: u16 = 8632;

/// The address the local proxy binds. Loopback only, so it needs no firewall
/// rule and is never reachable from the network.
const LOOPBACK: &str = "127.0.0.1";

/// The mDNS service type every NearbyPrint host registers under - the same
/// record a phone browses, so a Windows client discovers a host exactly the
/// way the phone app does.
const MDNS_SERVICE_TYPE: &str = "_ipp._tcp.local.";

/// The AirPrint service type: the plain `_ipp._tcp.local.` record *plus* the
/// `_universal` subtype Apple devices require (RFC 6763 §7.1).
///
/// macOS and iOS do not browse `_ipp._tcp.local.` when looking for a printer
/// to add - they browse `_universal._sub._ipp._tcp.local.`, the record every
/// real AirPrint device publishes. Answering only the parent type is what made
/// the printer invisible in the Mac's "Add Printer" dialog while a Windows
/// client on the same network found it fine.
///
/// `mdns-sd` accepts this form in `ServiceInfo::new`: it splits on `._sub.`,
/// registers under the parent `_ipp._tcp.local.` (so phones and Windows keep
/// discovering us exactly as before) *and* announces a PTR for the subtype,
/// answering queries for both. The instance fullname is unchanged
/// (`<name>._ipp._tcp.local.`), so unregistering and host discovery need no
/// special case. Because the announced A records come from the same
/// registration and `enable_addr_auto()` keeps them current, a host whose DHCP
/// lease changes is re-announced with the new address automatically - Mac and
/// mobile both follow the change without being re-added.
const AIRPRINT_SERVICE_TYPE: &str = "_universal._sub._ipp._tcp.local.";

// ---- IPP operation ids ----
const OP_PRINT_JOB: u16 = 0x0002;
const OP_PRINT_URI: u16 = 0x0003;
const OP_VALIDATE_JOB: u16 = 0x0004;
const OP_CREATE_JOB: u16 = 0x0005;
const OP_SEND_DOCUMENT: u16 = 0x0006;
const OP_CANCEL_JOB: u16 = 0x0008;
const OP_GET_JOB_ATTRS: u16 = 0x0009;
const OP_GET_JOBS: u16 = 0x000A;
const OP_GET_PRINTER_ATTRS: u16 = 0x000B;
const OP_PAUSE_PRINTER: u16 = 0x0010;
const OP_RESUME_PRINTER: u16 = 0x0011;
const OP_PURGE_JOBS: u16 = 0x0012;

// ---- IPP job states ----
/// RFC 8010 4.2.13 `job-state` values - the ones that actually go on the wire
/// in the mandatory `job-state` attribute (RFC 8011 4.3.3 / 4.3.4).
///
/// Kept apart from `JOB_*` below on purpose: those are this process's own
/// bookkeeping and their numbering does not match the RFC, so mixing the two
/// hands a spooler a state it has no meaning for.
const RFC_JOB_STATE_PENDING: i32 = 3;
const RFC_JOB_STATE_PROCESSING: i32 = 5;
const RFC_JOB_STATE_CANCELED: i32 = 7;
const RFC_JOB_STATE_ABORTED: i32 = 8;
const RFC_JOB_STATE_COMPLETED: i32 = 9;

/// Internal bookkeeping for `Job::ipp_state` - what this process thinks the
/// job is doing. Never sent to a client (see `RFC_JOB_STATE_*` above).
const JOB_PENDING: u16 = 1;
const JOB_PROCESSING: u16 = 3;
const JOB_CANCELED: u16 = 5;
const JOB_ABORTED: u16 = 6;
const JOB_COMPLETED: u16 = 7;

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
struct JobInfo {
    id: u32,
    name: String,
    device: String,
    user: String,
    pages: u32,
    bytes: u64,
    format: String,
    status: String,
    message: String,
    printer: String,
    file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_card: Option<bool>,
}

struct Job {
    info: JobInfo,
    path: Option<PathBuf>,
    ipp_state: u16,
    /// The copies count / orientation this job was submitted with, kept so
    /// the UI's own print button and the relay fallback print what the client
    /// asked for instead of one portrait copy.
    options: PrintOptions,
}

#[derive(Default)]
struct Jobs {
    map: Mutex<HashMap<u32, Job>>,
}

impl Jobs {
    fn update<F: FnOnce(&mut Job)>(&self, id: u32, f: F) -> Option<JobInfo> {
        let mut map = self.map.lock().unwrap();
        let job = map.get_mut(&id)?;
        f(job);
        Some(job.info.clone())
    }

    fn get_info(&self, id: u32) -> Option<JobInfo> {
        self.map.lock().unwrap().get(&id).map(|j| j.info.clone())
    }

    fn take_path(&self, id: u32) -> Option<PathBuf> {
        self.map.lock().unwrap().get_mut(&id)?.path.take()
    }
}

fn next_job_id() -> u32 {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// `YYYY-MM-DD HH:MM:SS` in UTC for a log line, from the wall clock.
///
/// No timezone crate is linked just for this: the civil-from-days conversion
/// is arithmetic, and a log is read next to the timestamps Windows itself
/// prints, so UTC is unambiguous enough.
fn log_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Howard Hinnant's civil_from_days.
    let days = (secs / 86_400) as i64;
    let clock = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        clock / 3600,
        (clock % 3600) / 60,
        clock % 60
    )
}

/// Append one already-formatted line to the log file, creating and rotating
/// it as needed. Never panics and never fails the caller: a full disk or a
/// read-only install directory costs the log line, not the print job.
fn log_to_file(line: &str) {
    use std::io::Write;

    // Next to the .exe on a normal install, then the temp directory when that
    // directory is not writable (e.g. Program Files).
    fn candidates() -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                out.push(dir.join("nearby-print.log"));
            }
        }
        out.push(std::env::temp_dir().join("nearby-print.log"));
        out
    }

    static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
    let mut slot = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        for path in candidates() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Rotate rather than grow without bound: this log takes a line per
            // HTTP request, and it must still show the first failure a week
            // from now.
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.len() > 4 * 1024 * 1024 {
                    let _ = std::fs::rename(&path, path.with_extension("log.old"));
                }
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                *slot = Some(file);
                break;
            }
        }
    }
    if let Some(file) = slot.as_mut() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// One line to stdout *and* to the log file, timestamped.
///
/// The installed build is a GUI-subsystem binary (`windows_subsystem =
/// "windows"`), so `println!` from it is discarded - every diagnosis of "the
/// job never arrived" has had to be re-run with a console build first. The
/// file half is what makes the shipped exe debuggable.
macro_rules! logln {
    ($($arg:tt)*) => {{
        let line = format!("[{}] {}", log_timestamp(), format!($($arg)*));
        println!("{line}");
        log_to_file(&line);
    }};
}

fn job_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("nearbyprint-jobs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Runs a helper program with no console window and waits at most `timeout`
/// for it. Returns `None` when the program is missing or wedges, so a stuck
/// spooler query or conversion can never hang the UI or a print job.
fn run_hidden(program: &Path, args: &[String], timeout: Duration) -> Option<std::process::Output> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cmd.output());
    });

    match rx.recv_timeout(timeout) {
        Ok(res) => res.ok(),
        Err(_) => {
            logln!("{} timed out after {timeout:?}", program.display());
            None
        }
    }
}

/// Spools `document` to the job folder and returns
/// `(path, filename, extension, size)`.
///
/// The name always ends in a printable extension - the sidecar rejects
/// anything that is not `.pdf`, so payloads we do not recognise are saved as
/// `.pdf` (what every client honouring our `document-format-supported` sends)
/// rather than the old, unusable `.bin`.
fn save_document_to_temp(document: &[u8], format: &str) -> (PathBuf, String, String, u64) {
    let dir = job_dir();
    let ext = extension_for(format, document);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!("nearby_print_{}_{}.{}", Uuid::new_v4().simple(), ts, ext);
    let path = dir.join(&filename);
    let _ = std::fs::write(&path, document);
    let size = document.len() as u64;
    (path, filename, ext.to_string(), size)
}

// ---------------------------------------------------------------------------
// App / server state
// ---------------------------------------------------------------------------

struct AppState {
    jobs: Arc<Jobs>,
    server_shutdown: Mutex<Option<mpsc::Sender<()>>>,
    /// Handle to the pure-Rust `mdns-sd` daemon. `Some` while we broadcast.
    mdns: Mutex<Option<ServiceDaemon>>,
    /// Handle to the *second* `mdns-sd` daemon, the one that browses for other
    /// hosts. Kept apart from `mdns` on purpose: discovery has to work on a PC
    /// that is not broadcasting at all, which is exactly the client role.
    discovery_daemon: Mutex<Option<ServiceDaemon>>,
    /// Every NearbyPrint host seen on the LAN, as last emitted to the UI.
    discovered_hosts: Arc<Mutex<Vec<DiscoveredHost>>>,
    /// Loopback proxy lifecycle, tracked separately from `server_shutdown`
    /// because the proxy outlives broadcast cycles - it is up from launch to
    /// exit so an already-installed Windows queue never finds a dead port.
    local_proxy_shutdown: Mutex<Option<mpsc::Sender<()>>>,
    /// Why the loopback proxy is not running, when it is not. Surfaced by
    /// `get_local_proxy_status` so the install button can say "port busy"
    /// instead of leaving Windows to report a connection refused.
    local_proxy_error: Mutex<String>,
    server_url: Mutex<String>,
    local_ip: Mutex<String>,
    printer_uuid: Mutex<String>,
    printer_name: Mutex<String>,
    /// Why the last attempt to bring the printer up failed, so the UI can say
    /// "port busy" instead of silently showing a toggle that will not stick.
    last_broadcast_error: Mutex<String>,
    /// Where documents received by this process are forwarded, or empty when
    /// this PC is the host and prints them itself. Shared with `IppState` so
    /// the UI can point the relay somewhere new without restarting the server.
    relay_host: Arc<Mutex<String>>,
    /// The queue relayed jobs are printed on. Empty means the machine's
    /// default printer, which is what `SumatraPDF -print-to-default` picks up.
    selected_printer: Arc<Mutex<String>>,
    /// Why the last relay attempt needed a fallback (or failed), so the UI can
    /// warn about a host that stopped answering instead of quietly printing
    /// somewhere the user did not choose. Cleared when a relay succeeds.
    relay_warning: Arc<Mutex<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        let default_name = format!("{} - {}", SERVICE_NAME_PREFIX, hostname());
        Self {
            jobs: Arc::new(Jobs::default()),
            server_shutdown: Mutex::new(None),
            mdns: Mutex::new(None),
            discovery_daemon: Mutex::new(None),
            discovered_hosts: Arc::new(Mutex::new(Vec::new())),
            local_proxy_shutdown: Mutex::new(None),
            local_proxy_error: Mutex::new(String::new()),
            server_url: Mutex::new(String::new()),
            local_ip: Mutex::new(String::new()),
            printer_uuid: Mutex::new(String::new()),
            printer_name: Mutex::new(default_name),
            last_broadcast_error: Mutex::new(String::new()),
            relay_host: Arc::new(Mutex::new(String::new())),
            selected_printer: Arc::new(Mutex::new(String::new())),
            relay_warning: Arc::new(Mutex::new(String::new())),
        }
    }
}

fn hostname() -> String {
    hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "PC".into())
}

#[derive(Clone)]
struct IppState {
    app: Option<tauri::AppHandle>,
    jobs: Arc<Jobs>,
    local_ip: String,
    /// The port *this* listener answers on: [`IPP_PORT`] for the network
    /// server, [`LOCAL_PROXY_PORT`] for the loopback proxy. Every URI handed
    /// back to a client (`printer-uri`, `job-uri`, `printer-more-info`) is
    /// built from it, so a reply can never point a spooler at the other
    /// listener's port and strand the job there.
    port: u16,
    printer_uuid: String,
    printer_name: String,
    /// See `AppState::relay_host`. Both roles of this binary share one server,
    /// so the forwarding target has to be readable from the IPP handler.
    relay_host: Arc<Mutex<String>>,
    /// See `AppState::selected_printer`.
    selected_printer: Arc<Mutex<String>>,
    /// See `AppState::relay_warning`. Shared the same way, so the IPP handler
    /// can record that a host stopped answering without the UI polling for it.
    relay_warning: Arc<Mutex<String>>,
}

// ---------------------------------------------------------------------------
// Network helpers
// ---------------------------------------------------------------------------

fn lan_ipv4s() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(ifaces) = list_afinet_netifas() {
        for (name, addr) in ifaces {
            if let IpAddr::V4(v4) = addr {
                let s = v4.to_string();
                if !v4.is_loopback() && !v4.is_link_local() {
                    let lower = name.to_lowercase();
                    // Exclude virtual adapters (VirtualBox, WSL, Hyper-V, Docker)
                    if !lower.contains("virtual")
                        && !lower.contains("vbox")
                        && !lower.contains("vethernet")
                        && !lower.contains("wsl")
                        && !lower.contains("docker")
                        && !s.starts_with("192.168.56.")
                    {
                        out.push(s);
                    }
                }
            }
        }
    }
    if out.is_empty() {
        if let Ok(ifaces) = list_afinet_netifas() {
            for (_name, addr) in ifaces {
                if let IpAddr::V4(v4) = addr {
                    if !v4.is_loopback() && !v4.is_link_local() {
                        out.push(v4.to_string());
                    }
                }
            }
        }
    }
    // Prioritize standard Wi-Fi router subnets
    out.sort_by_key(|ip| {
        if ip.starts_with("192.168.1.") || ip.starts_with("192.168.0.") || ip.starts_with("192.168.29.") || ip.starts_with("192.168.31.") {
            0
        } else if ip.starts_with("192.168.") {
            1
        } else if ip.starts_with("10.") {
            2
        } else {
            3
        }
    });
    out.dedup();
    out
}

fn is_private_ipv4(ip: &str) -> bool {
    if let Some(a) = ip.split('.').next() {
        match a {
            "10" => return true,
            "192" => return ip.starts_with("192.168."),
            "172" => {
                let second = ip.split('.').nth(1).and_then(|s| s.parse::<u8>().ok());
                if let Some(s) = second {
                    return (16..=31).contains(&s);
                }
            }
            _ => {}
        }
    }
    false
}

fn primary_lan_ip() -> String {
    let ips = lan_ipv4s();
    ips.iter()
        .find(|ip| is_private_ipv4(ip))
        .cloned()
        .or_else(|| ips.first().cloned())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

// ---------------------------------------------------------------------------
// Printers
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct PrinterInfo {
    name: String,
    uri: String,
    #[serde(rename = "isSelf")]
    is_self: bool,
    /// `false` when the spooler reports the queue as offline/paused, so the
    /// frontend can tuck it away behind the collapsed "Offline Printers" list.
    online: bool,
}

fn is_self_uri(uri: &str) -> bool {
    let lower = uri.to_lowercase();
    lower.contains("nearby%20print._ipp")
        || lower.contains("nearby%20print._ipps")
        || lower.contains("nearby print._ipp")
        || lower.contains("nearby print._ipps")
        || lower.contains(&format!(":{IPP_PORT}/"))
        || lower.contains(&format!(":{IPP_PORT}?"))
}

/// Whether `name` is a queue that feeds straight back into this process.
///
/// Printing to one of these is a loop: the job comes out of the spooler as an
/// IPP request, comes back in here, gets printed again - without end, and with
/// a page for every turn. `device_uri_of` catches it on CUPS, where the device
/// URI carries our own port. On Windows that helper shells out to `lpstat`,
/// which does not exist, so it has *always* returned nothing here - which is
/// why `is_self` has never been true for any queue on Windows and why the
/// self queue was happily accepted as the one relayed jobs print on.
///
/// The name is what identifies them without asking the spooler anything:
///
/// * `<device name>` - exactly what `Add to Windows` creates;
/// * [`VIRTUAL_PRINTER_NAME`] - the relay client's virtual printer;
/// * anything still beginning "NearbyPrint" - a queue left behind by a rename.
///   `update_printer_name` moves the advertisement but not the Windows queue,
///   so the old one keeps pointing at us under the old name.
fn is_our_own_queue(name: &str, device_name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.eq_ignore_ascii_case(device_name.trim()) {
        return true;
    }
    if trimmed.eq_ignore_ascii_case(VIRTUAL_PRINTER_NAME) {
        return true;
    }
    let mine = SERVICE_NAME_PREFIX.to_ascii_lowercase();
    trimmed.to_ascii_lowercase().starts_with(&mine)
}

fn discover_printers(device_name: &str) -> Vec<PrinterInfo> {
    let mut list = Vec::new();
    if let Ok(out) = Command::new("lpstat").arg("-v").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("device for ") {
                    if let Some((name, uri)) = rest.split_once(": ") {
                        list.push(PrinterInfo {
                            name: name.trim().to_string(),
                            uri: uri.trim().to_string(),
                            is_self: is_self_uri(uri.trim())
                                || is_our_own_queue(name.trim(), device_name),
                            online: true,
                        });
                    }
                }
            }
        }
    }

    // `lpstat -p` flags the queues that are disabled; everything else it
    // reports as idle or printing. Skipped when `lpstat` is not available
    // (Windows), where the spooler below already gives us a state.
    if !list.is_empty() {
        if let Ok(out) = Command::new("lpstat").arg("-p").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for p in list.iter_mut() {
                    let head = format!("printer {} ", p.name);
                    if let Some(line) = text.lines().find(|l| l.starts_with(&head)) {
                        if line.contains("disabled") {
                            p.online = false;
                        }
                    }
                }
            }
        }
    }

    if list.is_empty() {
        for p in printers::get_printers() {
            // Baseline from winspool. Its `Status` is 0 (idle) for nearly
            // every queue - even ones Windows itself shows as "Offline" - so
            // this is only a fallback for when the query below fails.
            let online = !matches!(p.state, PrinterState::OFFLINE | PrinterState::PAUSED);
            // winspool hands back no device URI, so a queue belonging to this
            // process can only be recognized by its name - and it has to be
            // recognized, or the front end offers it as somewhere to print
            // *to*, which is the same as offering an infinite loop.
            let is_self = is_our_own_queue(&p.name, device_name);
            list.push(PrinterInfo {
                name: p.name,
                uri: String::new(),
                is_self,
                online,
            });
        }

        // Ask Windows itself (Win32_Printer) whether each queue is really
        // reachable, and overwrite the guess above with the real answer.
        let status = platform_printer_statuses();
        if !status.is_empty() {
            for p in list.iter_mut() {
                if let Some(&online) = status.get(&p.name.to_lowercase()) {
                    p.online = online;
                }
            }
        }
    }
    list
}

/// Raw `Win32_Printer` fields, exactly as PowerShell hands them back.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Win32PrinterStatus {
    name: String,
    work_offline: Option<bool>,
    printer_status: Option<u32>,
}

/// One hidden PowerShell round-trip that reports every installed queue.
/// `Get-Printer` cannot be used here: on Windows 10 its `PrinterStatus` is 0
/// and `WorkOffline` is null for *all* printers, which would show everything
/// as online. `Get-CimInstance Win32_Printer` carries the real values.
const PS_PRINTER_STATUS_SCRIPT: &str = concat!(
    "$rows = @(Get-CimInstance -ClassName Win32_Printer | ",
    "Select-Object Name,WorkOffline,PrinterStatus); ",
    "ConvertTo-Json -InputObject $rows -Compress"
);

/// `WorkOffline` is the "Use Printer Offline" flag Windows itself renders as
/// an "Offline" badge; `PrinterStatus` 6 = stopped printing, 7 = offline
/// (MSDN Win32_Printer). Anything unknown stays online so a usable queue can
/// never end up hidden in the collapsed list.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn is_printer_online(work_offline: Option<bool>, printer_status: Option<u32>) -> bool {
    if work_offline == Some(true) {
        return false;
    }
    !matches!(printer_status, Some(6) | Some(7))
}

/// Lower-cased printer name -> online flag.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_printer_statuses(stdout: &str) -> HashMap<String, bool> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return HashMap::new();
    }
    // PowerShell serialises a lone printer as an object instead of a list.
    let json = if trimmed.starts_with('[') {
        trimmed.to_string()
    } else {
        format!("[{trimmed}]")
    };
    let rows: Vec<Win32PrinterStatus> = match serde_json::from_str(&json) {
        Ok(rows) => rows,
        Err(e) => {
            logln!("Printer status query returned unparseable JSON: {e}");
            return HashMap::new();
        }
    };
    rows.into_iter()
        .map(|r| (r.name.to_lowercase(), is_printer_online(r.work_offline, r.printer_status)))
        .collect()
}

/// Runs the status query with a hard timeout, so a wedged spooler can never
/// freeze the printer list.
#[cfg(windows)]
fn platform_printer_statuses() -> HashMap<String, bool> {
    let mut args: Vec<String> = ["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push(PS_PRINTER_STATUS_SCRIPT.to_string());

    match run_hidden(Path::new("powershell"), &args, Duration::from_secs(8)) {
        Some(o) if o.status.success() => {
            parse_printer_statuses(&String::from_utf8_lossy(&o.stdout))
        }
        Some(o) => {
            logln!(
                "Printer status query failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            HashMap::new()
        }
        None => {
            logln!("Printer status query could not run or timed out");
            HashMap::new()
        }
    }
}

#[cfg(not(windows))]
fn platform_printer_statuses() -> HashMap<String, bool> {
    HashMap::new()
}

fn device_uri_of(printer: &str) -> Option<String> {
    let out = Command::new("lpstat").arg("-v").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("device for ") {
            if let Some((name, uri)) = rest.split_once(": ") {
                if name.trim() == printer {
                    return Some(uri.trim().to_string());
                }
            }
        }
    }
    None
}

/// Runs on a blocking thread: the Win32_Printer status query takes about a
/// second, and this must not stall the UI thread while the list loads.
#[tauri::command]
async fn get_printers(state: tauri::State<'_, AppState>) -> Result<Vec<PrinterInfo>, String> {
    // The device name goes along so the list can say which queue belongs to
    // this process - that is the one queue the user must never be offered as
    // a place to print.
    let device_name = state.printer_name.lock().unwrap().clone();
    tokio::task::spawn_blocking(move || discover_printers(&device_name))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_available_printers(state: tauri::State<'_, AppState>) -> Result<Vec<PrinterInfo>, String> {
    get_printers(state).await
}

#[derive(Serialize, Clone)]
struct NetworkDetail {
    ip: String,
    all_ips: Vec<String>,
    hostname: String,
    hostname_url: String,
}

#[tauri::command]
fn get_network_details() -> Result<NetworkDetail, String> {
    let host = hostname();
    let ips = lan_ipv4s();
    let primary = ips.first().cloned().unwrap_or_else(|| "127.0.0.1".into());
    Ok(NetworkDetail {
        ip: primary,
        all_ips: ips,
        hostname: host.clone(),
        hostname_url: format!("http://{}.local:{}/portal", host, IPP_PORT),
    })
}

#[tauri::command]
fn check_firewall_status() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = Command::new("netsh")
            .args(["advfirewall", "firewall", "show", "rule", "name=NearbyPrint - allow TCP 8631", "verbose"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        if let Ok(output) = out {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains("NearbyPrint - allow TCP 8631") && text.contains("Enabled:                              Yes") {
                for line in text.lines() {
                    if let Some(val) = line.strip_prefix("Program:") {
                        let trimmed = val.trim();
                        if !trimmed.eq_ignore_ascii_case("Any") && !trimmed.is_empty() {
                            return false;
                        }
                    }
                }
                return true;
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[tauri::command]
async fn fix_windows_firewall() -> Result<String, String> {
    #[cfg(windows)]
    {
        let script = r#"
            $ErrorActionPreference = 'Stop'
            netsh advfirewall firewall delete rule name="NearbyPrint - allow TCP 8631" | Out-Null
            netsh advfirewall firewall add rule name="NearbyPrint - allow TCP 8631" dir=in action=allow protocol=TCP localport=8631 profile=any enable=yes
            netsh advfirewall firewall delete rule name="NearbyPrint - allow UDP 5353" | Out-Null
            netsh advfirewall firewall add rule name="NearbyPrint - allow UDP 5353" dir=in action=allow protocol=UDP localport=5353 profile=any enable=yes
            netsh advfirewall firewall delete rule name="NearbyPrint IPP" | Out-Null
            netsh advfirewall firewall add rule name="NearbyPrint IPP" dir=in action=allow protocol=TCP localport=8631,8632 profile=any enable=yes
            Write-Output "OK: Windows Firewall opened for NearbyPrint (TCP 8631, UDP 5353)"
        "#;
        run_elevated_script("NearbyPrint-FirewallFix.ps1", script)
    }
    #[cfg(not(windows))]
    {
        Ok("Firewall rules are only required on Windows".to_string())
    }
}

#[tauri::command]
fn get_local_ips() -> Result<Vec<String>, String> {
    Ok(lan_ipv4s())
}

#[tauri::command]
fn get_printer_name(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(state.printer_name.lock().unwrap().clone())
}

/// The name is the user's, not ours: whatever they last typed stays the
/// printer's name across restarts instead of being recomputed from the
/// hostname on the next launch.
#[tauri::command]
async fn update_printer_name(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    new_name: String,
) -> Result<String, String> {
    let trimmed = new_name.trim().to_string();
    if trimmed.is_empty() {
        return Err("Printer name cannot be empty".into());
    }

    // Tear down under the *old* name first: mDNS has to unregister the service
    // it actually registered, and everything built after this point - the mDNS
    // record, `printer-name`, the setup script - must carry the new one.
    let was_broadcasting = !state.server_url.lock().unwrap().is_empty();
    if was_broadcasting {
        stop_broadcast(&state);
    }

    *state.printer_name.lock().unwrap() = trimmed.clone();
    write_config(&app, PRINTER_NAME_FILE, &trimmed);

    // The loopback proxy answers with this name too, so it is rebuilt the same
    // way the broadcast below is. Restarting it here also lands in the window
    // where its old listener is still closing, which is what the bind retry in
    // `start_local_proxy` exists for.
    let proxy_up = state
        .local_proxy_shutdown
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(false);
    if proxy_up {
        stop_local_proxy(&state);
        if let Err(e) = start_local_proxy(&app).await {
            logln!("name changed, but the local proxy could not restart: {e}");
        }
    }

    // Re-register rather than just `start_mdns_sd`: that call handed back a
    // daemon nobody kept, so the advertisement disappeared as soon as the
    // temporary went out of scope - a rename silently ended broadcasting.
    if was_broadcasting {
        if let Err(e) = start_broadcasting(app, state).await {
            logln!("name changed, but broadcasting could not restart: {e}");
        }
    }
    Ok(format!("Printer name updated to '{}'", trimmed))
}

/// The PowerShell payload behind `generate_client_script`.
///
/// Windows has exactly two command-line entry points that can install a queue
/// pointing at an HTTP IPP URL, and the script tries both in order:
///
/// 1. `Add-Printer -Name <n> -IppURL <url>` maps to
///    `MSFT_Printer.AddByAdaptivePort`, the entry point that creates an
///    *adaptive* ("IPP Port") port straight from a URL. Windows probes the
///    address and binds the Microsoft IPP Class Driver itself, so no driver
///    selection happens at all. Present on Windows 10 1903+ / 11. First
///    choice.
/// 2. `rundll32 printui.dll,PrintUIEntry /if /b <n> /r <url> /m <driver>` for
///    builds without `-IppURL`. Microsoft's documented
///    no-user-interaction path: it creates the same URL port with the
///    mandatory driver and never opens the Add Printer wizard either.
///
/// Deliberately *not* `Add-PrinterPort`, even though that reads like the
/// obvious command: the cmdlet only exposes local / LPR / Standard TCP/IP
/// ports (`MSFT_PrinterPortTasks` offers `AddByLocalPort`, `AddByLprPort`,
/// `AddByTcpPort`, `AddByTcpPortLprMode` - there is no IPP variant), so
///
///     Add-PrinterPort -Name $p -PrinterHostAddress "http://host:8631/ipp/print"
///
/// records a RAW TCP port on port 9100 whose "host" is the literal URL.
/// Verified on Windows 11: it comes back as `MSFT_TcpIpPrinterPort`,
/// `Protocol = RAW`, `PortMonitor = TCPMON.DLL` - it installs happily and can
/// never deliver a byte to an HTTP server, which is precisely the silent
/// failure this script exists to avoid.
///
/// The script probes the port first (so a stopped broadcast fails with an
/// actionable message instead of a spooler error), re-runs itself elevated
/// (both paths need administrator), is idempotent, and never opens a wizard.
const CLIENT_SCRIPT_TEMPLATE: &str = r#"# ---------------------------------------------------------------------------
# Nearby Print - one-click printer setup for Windows 10 / 11
# Generated for {{PRINTER_NAME}} at {{IPP_URL}}
#
#   Right-click this file -> Run with PowerShell
#   or: powershell -NoProfile -ExecutionPolicy Bypass -File .\NearbyPrint-Setup.ps1
# ---------------------------------------------------------------------------
[CmdletBinding()]
param(
    # Duplicates every status line into this file. The app passes it when it
    # runs this script elevated: an elevated PowerShell owns its own console,
    # so its output would otherwise never reach the caller.
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'

$Url    = '{{IPP_URL}}'
$Server = '{{IPP_HOST}}'
$Name   = '{{PRINTER_NAME}}'
$Driver = 'Microsoft IPP Class Driver'

function Say($Text) {
    if ($LogPath) { Add-Content -LiteralPath $LogPath -Value $Text }
}
function Note($Text) { Write-Host ('  ' + $Text) -ForegroundColor DarkGray; Say $Text }
function Good($Text) { Write-Host ('  OK   ' + $Text) -ForegroundColor Green; Say ('OK   ' + $Text) }
function Bad($Text)  { Write-Host ('  FAIL ' + $Text) -ForegroundColor Red;  Say ('FAIL ' + $Text) }

# Add-Printer and printui both require administrator rights. This file simply
# relaunches itself elevated; the UAC prompt is the only click needed.
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
        Bad ('setup was cancelled or refused: ' + $_.Exception.Message)
        exit 2
    }
}

Write-Host ''
Write-Host ('Nearby Print - adding ' + $Name) -ForegroundColor Cyan
Write-Host ('  Server: ' + $Url)

# The sharing PC must be broadcasting before Windows can probe it. Failing
# here beats a spooler error 30 seconds later with no hint what went wrong.
$Reachable = $false
try {
    $Client  = New-Object Net.Sockets.TcpClient
    $Pending = $Client.BeginConnect($Server, 8631, $null, $null)
    $Reachable = $Pending.AsyncWaitHandle.WaitOne(3000, $false)
    if ($Reachable) { $Client.EndConnect($Pending) }
    $Client.Close()
} catch {
    $Reachable = $false
}

if (-not $Reachable) {
    Write-Host ''
    Bad ('Nearby Print is not reachable at ' + $Url)
    Write-Host '  Turn on "Wireless Broadcasting" on the sharing PC (and make sure'
    Write-Host '  this PC is on the same Wi-Fi network), then run this script again.'
    Write-Host ''
    exit 1
}

# Already installed? Report it and stop rather than fighting over the queue.
$Existing = Get-Printer -Name $Name -ErrorAction SilentlyContinue
if ($Existing) {
    Good ("already installed - port '" + $Existing.PortName + "', driver '" + $Existing.DriverName + "'")
    Write-Host ''
    exit 0
}

$Added = $false

# 1. The real IPP port, built from the URL. `Add-Printer -IppURL` calls
#    MSFT_Printer.AddByAdaptivePort: Windows probes the address, creates an
#    "IPP Port" pointing at $Url and binds the Microsoft IPP Class Driver
#    itself - no wizard, no driver list, nothing for the user to pick.
try {
    Add-Printer -Name $Name -IppURL $Url -ErrorAction Stop | Out-Null
    $Added = $true
    Good ('added with an IPP port targeting ' + $Url)
} catch {
    Note ('Add-Printer -IppURL unavailable: ' + $_.Exception.Message)
}

# 2. Builds without -IppURL: printui's /if flow creates the same URL port with
#    the mandatory driver. It is Microsoft's documented no-user-interaction
#    entry point, so it stays silent and never opens the Add Printer wizard.
#    The queue appears asynchronously, hence the short wait for it.
if (-not $Added) {
    & rundll32.exe 'printui.dll,PrintUIEntry' /if /b $Name /r $Url /m $Driver
    for ($i = 0; $i -lt 20 -and -not (Get-Printer -Name $Name -ErrorAction SilentlyContinue); $i++) {
        Start-Sleep -Milliseconds 500
    }
    if (Get-Printer -Name $Name -ErrorAction SilentlyContinue) {
        $Added = $true
        Good ('added with driver "' + $Driver + '"')
    } else {
        Note 'printui did not install the queue'
    }
}

if (-not $Added) {
    Write-Host ''
    Bad 'could not add the printer automatically'
    Write-Host '  Add it by hand (the full URL is what makes Windows pick an IPP port):'
    Write-Host '    Settings > Bluetooth & devices > Printers & scanners > Add device'
    Write-Host '    > The printer that I want isn'"'"'t listed'
    Write-Host '    > Select a shared printer by name'
    Write-Host ('    ' + $Url)
    Write-Host ('    Driver if Windows asks: ' + $Driver)
    Write-Host ''
    exit 1
}

$Printer = Get-Printer -Name $Name
$Port = Get-PrinterPort -Name $Printer.PortName -ErrorAction SilentlyContinue
if ($Port -and $Port.Description -match 'IPP|Internet') {
    Good ('port: ' + $Printer.PortName + ' (' + $Port.Description + ')')
} else {
    Note ('port: ' + $Printer.PortName)
}

if ($Printer.DriverName -eq $Driver) {
    Good ('driver: ' + $Printer.DriverName)
} else {
    Note ("installed with driver '" + $Printer.DriverName + "'")
}

Write-Host ''
Write-Host ('Done - ' + $Name + ' is ready to use.') -ForegroundColor Green
Write-Host ''
exit 0
"#;

/// The Windows queue that lets `Ctrl+P` reach this app's own loopback proxy.
///
/// Everything here is *local*: the port points at `127.0.0.1:8632`, the proxy
/// this app keeps listening on from launch to exit, so the job never leaves
/// the machine until the Rust handler forwards it. That is the whole reason
/// for this path - the adaptive/IPP port machinery that Windows uses for a
/// remote URL is the part that fails on this build, and a loopback address
/// gives it nothing to discover, resolve or cache.
///
/// The proxy deliberately does *not* share [`IPP_PORT`] with the network
/// server: both run in this one process on a client PC, and two listeners on
/// one port is the `os error 10048` that used to leave the queue pointing at
/// nothing.
///
/// Same shape as `CLIENT_SCRIPT_TEMPLATE`: probe first, re-run elevated,
/// idempotent, never open a wizard. It runs on the *client* PC, so the probe
/// is this machine's own port rather than a peer's.
const LOCAL_PRINTER_SCRIPT: &str = r#"# ---------------------------------------------------------------------------
# Nearby Print - local "virtual printer" for the relay client
# Generated for {{PRINTER_NAME}} at {{IPP_URL}}
# ---------------------------------------------------------------------------
[CmdletBinding()]
param(
    # Duplicates every status line into this file: an elevated PowerShell owns
    # its own console, so its output would otherwise never reach the caller.
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'

$Name      = '{{PRINTER_NAME}}'
$Url       = '{{IPP_URL}}'
$Driver    = 'Microsoft IPP Class Driver'
$Loopback  = '127.0.0.1'
$Port      = {{PROXY_PORT}}

function Say($Text) {
    if ($LogPath) { Add-Content -LiteralPath $LogPath -Value $Text }
}
function Note($Text) { Write-Host ('  ' + $Text) -ForegroundColor DarkGray; Say $Text }
function Good($Text) { Write-Host ('  OK   ' + $Text) -ForegroundColor Green; Say ('OK   ' + $Text) }
function Bad($Text)  { Write-Host ('  FAIL ' + $Text) -ForegroundColor Red;  Say ('FAIL ' + $Text) }

# Add-Printer and printui both need administrator. Relaunching this file is
# the whole elevation story - one UAC prompt, nothing to type.
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
        Bad ('setup was cancelled or refused: ' + $_.Exception.Message)
        exit 2
    }
}

Write-Host ''
Write-Host ('Nearby Print - installing ' + $Name) -ForegroundColor Cyan
Write-Host ('  Local endpoint: ' + $Url)

# 1. The queue only works while this app's loopback proxy is up. The proxy
#    starts with the app itself - no toggle to remember - so a failure here
#    means Nearby Print is not running, which is worth saying before Windows
#    is asked to create anything.
$Up = $false
try {
    $Client  = New-Object Net.Sockets.TcpClient
    $Pending = $Client.BeginConnect($Loopback, $Port, $null, $null)
    $Up = $Pending.AsyncWaitHandle.WaitOne(3000, $false)
    if ($Up) { $Client.EndConnect($Pending) }
    $Client.Close()
} catch {
    $Up = $false
}
if (-not $Up) {
    Write-Host ''
    Bad ('the local print server is not listening on ' + $Loopback + ':' + $Port)
    Write-Host '  Open Nearby Print on this PC so the local proxy can start, then run this again.'
    Write-Host ''
    exit 1
}

# 2. The driver has to exist before a URL port can bind to it.
if (-not (Get-PrinterDriver -Name $Driver -ErrorAction SilentlyContinue)) {
    Write-Host ''
    Bad ("driver '" + $Driver + "' is not installed on this PC")
    Write-Host '  It ships with Windows 10/11. If it is missing, run Windows Update first.'
    Write-Host ''
    exit 1
}

# 3. Re-running must repair rather than fail: a queue left pointing at an old
#    address is exactly the state this button is used to escape.
$Existing = Get-Printer -Name $Name -ErrorAction SilentlyContinue
if ($Existing) {
    $PortObj = Get-PrinterPort -Name $Existing.PortName -ErrorAction SilentlyContinue
    $CurrentUri = ''
    if ($PortObj -and ($PortObj.PSObject.Properties.Name -contains 'PrinterHostUri')) {
        $CurrentUri = [string]$PortObj.PrinterHostUri
    }
    if ($CurrentUri -and ($CurrentUri.TrimEnd('/') -ieq $Url.TrimEnd('/'))) {
        Good ("already installed - port '" + $Existing.PortName + "', driver '" + $Existing.DriverName + "'")
        Write-Host ''
        exit 0
    }
    Note ("replacing the queue on port '" + $Existing.PortName + "'")
    try {
        Remove-Printer -Name $Name -ErrorAction Stop
    } catch {
        Write-Host ''
        Bad ('could not replace the existing queue: ' + $_.Exception.Message)
        exit 1
    }
}

# 4. Create the queue. `-IppURL` asks Windows to build the IPP port itself;
#    the printui path is Microsoft's silent fallback when that cmdlet is
#    unavailable or refuses. Both are reported honestly - a half-created
#    queue is worse than none, so the failure text is what comes back to the app.
$Added    = $false
$LastFail = ''

try {
    Add-Printer -Name $Name -DriverName $Driver -IppURL $Url -ErrorAction Stop | Out-Null
    $Added = $true
    Good ('added with an IPP port targeting ' + $Url)
} catch {
    $LastFail = $_.Exception.Message
    Note ('Add-Printer -IppURL failed: ' + $LastFail)
    try {
        Add-Printer -Name $Name -IppURL $Url -ErrorAction Stop | Out-Null
        $Added = $true
        Good ('added with an IPP port targeting ' + $Url)
    } catch {
        $LastFail = $_.Exception.Message
        Note ('Add-Printer -IppURL (without -DriverName) failed: ' + $LastFail)
    }
}

if (-not $Added) {
    & rundll32.exe 'printui.dll,PrintUIEntry' /if /b $Name /r $Url /m $Driver
    for ($i = 0; $i -lt 20 -and -not (Get-Printer -Name $Name -ErrorAction SilentlyContinue); $i++) {
        Start-Sleep -Milliseconds 500
    }
    if (Get-Printer -Name $Name -ErrorAction SilentlyContinue) {
        $Added = $true
        Good ('added via printui /if targeting ' + $Url)
    }
}

if (-not $Added) {
    Write-Host ''
    Bad ('Windows refused to create the queue for ' + $Url)
    if ($LastFail) { Write-Host ('  ' + $LastFail) }
    Write-Host '  Windows reports this in Event Viewer under'
    Write-Host '  Applications and Services Logs > Microsoft > Windows > PrintService > Admin.'
    Write-Host ''
    exit 1
}

$Printer = Get-Printer -Name $Name
Note ('port: ' + $Printer.PortName)
Note ('driver: ' + $Printer.DriverName)

Write-Host ''
Write-Host ('Done - ' + $Name + ' is ready. Press Ctrl+P anywhere and pick it.') -ForegroundColor Green
Write-Host ''
exit 0
"#;

/// Fills `CLIENT_SCRIPT_TEMPLATE` for `ip` / `printer_name`.
fn powershell_setup_script(ip: &str, printer_name: &str) -> String {
    // The name is embedded in single quotes, where PowerShell escapes a
    // literal quote by doubling it.
    let name = printer_name.trim().replace('\'', "''");
    let name = if name.is_empty() {
        "NearbyPrint".to_string()
    } else {
        name
    };
    CLIENT_SCRIPT_TEMPLATE
        .replace("{{IPP_URL}}", &format!("http://{ip}:{IPP_PORT}/ipp/print"))
        .replace("{{IPP_HOST}}", ip)
        .replace("{{PRINTER_NAME}}", &name)
}

/// Builds the one-click Windows setup script for this PC.
///
/// Returns the complete `.ps1` text. The client pastes it, saves it, runs it -
/// no Add Printer wizard, no driver hunting, and no dependence on Windows
/// discovering us over the network first.
#[tauri::command]
fn generate_client_script(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let ip = {
        let ip = state.local_ip.lock().unwrap().trim().to_string();
        if ip.is_empty() {
            primary_lan_ip()
        } else {
            ip
        }
    };
    if ip.is_empty() {
        return Err("No LAN address yet - turn broadcasting on first".into());
    }
    let name = state.printer_name.lock().unwrap().clone();
    Ok(powershell_setup_script(&ip, &name))
}

/// What the UI needs to repaint itself after a reload: whether this machine is
/// sharing its printer right now, and if not why not.
#[derive(Serialize)]
struct BroadcastStatus {
    broadcasting: bool,
    url: String,
    /// Empty while broadcasting is up or was never attempted.
    last_error: String,
}

#[tauri::command]
fn get_broadcast_status(state: tauri::State<'_, AppState>) -> BroadcastStatus {
    let url = state.server_url.lock().unwrap().trim().to_string();
    let last_error = if url.is_empty() {
        state
            .last_broadcast_error
            .lock()
            .unwrap()
            .trim()
            .to_string()
    } else {
        String::new()
    };
    BroadcastStatus {
        broadcasting: !url.is_empty(),
        url,
        last_error,
    }
}

#[derive(Serialize)]
struct ServerStatusResponse {
    pub running: bool,
    pub broadcasting: bool,
    pub url: String,
    pub error: String,
    pub last_error: String,
}

#[tauri::command]
fn get_server_status(state: tauri::State<'_, AppState>) -> ServerStatusResponse {
    let bs = get_broadcast_status(state);
    ServerStatusResponse {
        running: bs.broadcasting,
        broadcasting: bs.broadcasting,
        url: bs.url,
        error: bs.last_error.clone(),
        last_error: bs.last_error,
    }
}

/// Installs this PC's own Windows queue against the server that is running now.
///
/// Windows is the one client that cannot be aimed at a bare URL from the UI:
/// the queue has to be created with `Add-Printer -IppURL`, which needs exactly
/// one administrator approval. This runs the same script the product has
/// always generated, but elevated and silent, and brings the result back to a
/// single button - no script to copy, no URL to edit, no Add Printer wizard.
#[tauri::command]
async fn add_windows_printer(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let url = state.server_url.lock().unwrap().trim().to_string();
    if url.is_empty() {
        return Err("Turn on Wireless Broadcasting first - Windows has nothing to probe.".into());
    }

    let ip = {
        let ip = state.local_ip.lock().unwrap().trim().to_string();
        if ip.is_empty() {
            primary_lan_ip()
        } else {
            ip
        }
    };
    if ip.is_empty() {
        return Err("No LAN address yet - turn broadcasting on first.".into());
    }
    let name = state.printer_name.lock().unwrap().clone();

    run_elevated_script("NearbyPrint-Setup.ps1", &powershell_setup_script(&ip, &name))
}

/// Writes `script` to the setup folder, runs it through PowerShell - the
/// script elevates itself, so this is the one UAC prompt - and folds the
/// `OK` / `FAIL` lines it logged into a single message for the UI.
///
/// Shared by both install buttons: they differ only in the script they hand
/// over, and the "how do we get the elevated half's output back?" problem is
/// identical for both.
fn run_elevated_script(file_name: &str, script: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join("nearby-print-setup");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot use that folder: {e}"))?;
    let script_path = dir.join(file_name);
    let log_path = dir.join("setup.log");
    let _ = std::fs::remove_file(&log_path);
    std::fs::write(&script_path, script)
        .map_err(|e| format!("Cannot write the setup script: {e}"))?;

    // Hidden, because the script re-launches itself elevated and that second
    // console would otherwise flash across the desktop. `-LogPath` is how the
    // elevated half talks back to us.
    let path = script_path.to_string_lossy().to_string();
    let log = log_path.to_string_lossy().to_string();
    let mut child = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-File")
        .arg(&path)
        .arg("-LogPath")
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start PowerShell: {e}"))?;

    let status = child
        .wait()
        .map_err(|e| format!("Setup did not finish: {e}"))?;

    let logged = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .trim()
        .to_string();

    // The script's own status lines are the only part worth showing; the rest
    // is banner text. The *first* OK is the headline ("added with an IPP port
    // targeting ...") while the *last* FAIL is the one that ended the run.
    let status_line = |prefix: &str, use_last: bool| {
        let matches: Vec<&str> = logged
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(prefix))
            .collect();
        let line = if use_last { matches.last() } else { matches.first() };
        line.map(|line| line[prefix.len()..].trim().to_string())
    };

    if status.success() {
        return Ok(status_line("OK", false).unwrap_or_else(|| "Installed.".into()));
    }
    if let Some(message) = status_line("FAIL", true) {
        return Err(message);
    }
    if logged.is_empty() {
        // No status line at all: the UAC prompt was dismissed.
        return Err("Setup was cancelled, or Windows refused to run it.".into());
    }
    Err(logged.lines().next_back().unwrap_or("Setup failed.").into())
}

/// The queue `Ctrl+P` offers on a client PC. Deliberately not the same string
/// as any queue the host side installs: one machine may do both, and two
/// queues with one name would fight over which port they point at.
const VIRTUAL_PRINTER_NAME: &str = "NearbyPrint (Local)";

/// Installs the loopback queue that lets any Windows app print into this one.
///
/// The queue always targets `http://127.0.0.1:8632/ipp/print` - the local
/// proxy - and never a host's address: which PC to print on lives in
/// `relay-host` and is read the moment a document arrives, so pointing the
/// relay at a different discovered host needs no reinstall and no second UAC
/// prompt. The IP picked from the discovered list is saved by the caller
/// before this runs; nothing about the destination is baked into the queue.
#[tauri::command]
async fn install_virtual_printer(app: tauri::AppHandle) -> Result<String, String> {
    // The queue is only worth creating while something listens behind it, and
    // the proxy can have failed to start at launch (its port taken by another
    // copy of this app). Bringing it up here is what makes one click repair
    // the whole path instead of leaving Windows with a dead queue.
    start_local_proxy(&app).await?;

    // The queue speaks IPP, so it points at the IPP path - *not* at
    // `relay_url`, which is where the Rust handler forwards the bytes to once
    // they are already here. Mixing the two would install a port that POSTs
    // its documents to an endpoint that does not answer IPP.
    let url = local_proxy_url();
    let script = LOCAL_PRINTER_SCRIPT
        .replace("{{IPP_URL}}", &url)
        .replace("{{PROXY_PORT}}", &LOCAL_PROXY_PORT.to_string())
        .replace("{{PRINTER_NAME}}", VIRTUAL_PRINTER_NAME);
    run_elevated_script("NearbyPrint-VirtualPrinter.ps1", &script)
}

/// What the relay is currently doing, for the UI to render.
#[derive(Serialize)]
struct RelayStatus {
    /// Exactly what the user typed, so the field round-trips unchanged.
    host: String,
    /// The URL documents are posted to. Empty when no host is configured.
    url: String,
    /// Whether this process is forwarding instead of printing locally.
    forwarding: bool,
    /// Non-empty when the configured host could not be reached and jobs are
    /// being printed on this PC instead. Empty otherwise.
    warning: String,
}

#[tauri::command]
fn get_relay_status(state: tauri::State<'_, AppState>) -> RelayStatus {
    let host = state.relay_host.lock().unwrap().trim().to_string();
    let forwarding = !host.is_empty();
    let url = relay_url(&host).unwrap_or_default();
    let warning = state.relay_warning.lock().unwrap().clone();
    RelayStatus {
        url,
        host,
        forwarding,
        warning,
    }
}

/// Points the relay at a host (or clears it with an empty string) and
/// persists the choice, so a client PC comes back up forwarding after a
/// restart just like the broadcast toggle does.
#[tauri::command]
fn set_relay_host(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    host: String,
) -> Result<RelayStatus, String> {
    let trimmed = host.trim().to_string();
    let url = if trimmed.is_empty() {
        String::new()
    } else {
        relay_url(&trimmed)?
    };
    write_config(&app, RELAY_HOST_FILE, &trimmed);
    *state.relay_host.lock().unwrap() = trimmed.clone();
    // Pointing at a different host is a fresh claim: any earlier "unreachable"
    // was about the host just replaced, and the new one has not been tried.
    *state.relay_warning.lock().unwrap() = String::new();
    let forwarding = !url.is_empty();
    if !forwarding {
        logln!("Relay off - this PC prints what it receives itself");
    } else {
        logln!("Relay on - received documents are forwarded to {url}");
    }
    Ok(RelayStatus {
        host: trimmed,
        url,
        forwarding,
        warning: String::new(),
    })
}

/// The local queue relayed jobs print on, or `""` for the Windows default.
#[tauri::command]
fn get_selected_printer(state: tauri::State<'_, AppState>) -> String {
    state.selected_printer.lock().unwrap().trim().to_string()
}

#[tauri::command]
fn set_selected_printer(app: tauri::AppHandle, state: tauri::State<'_, AppState>, printer: String) {
    let trimmed = printer.trim().to_string();
    write_config(&app, SELECTED_PRINTER_FILE, &trimmed);
    *state.selected_printer.lock().unwrap() = trimmed;
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// The bundled sidecar that prints a document without ever opening a window.
///
/// Declared as `bin/SumatraPDF` under `bundle.externalBin` in
/// `tauri.conf.json`, which makes `tauri-build` copy
/// `src-tauri/bin/SumatraPDF-x86_64-pc-windows-msvc.exe` next to the built
/// application as `SumatraPDF.exe` on every build (and into the resources
/// folder when an installer is produced).
///
/// SumatraPDF (portable) rather than a third-party `PDFtoPrinter` build: it is
/// a single statically linked executable whose import table lists nothing but
/// operating-system DLLs, so it starts on a clean Windows install with no
/// Visual C++ redistributable, no runtime installer and no PDF viewer.
const SIDECAR_EXE: &str = "SumatraPDF.exe";

/// How long a silent print may take before we stop waiting on it. A typical
/// job returns in a second or two; the bound only exists so a wedged spooler
/// can never leave a print thread hanging forever.
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(120);

/// The two job-template values a client asks for and the print engine has to
/// honour: how many copies, and which way the page turns.
///
/// Parsed out of the IPP request's job-template group (`copies` and
/// `orientation-requested`, RFC 8011 4.2.14) and carried all the way to
/// SumatraPDF - previously both were read from the wire and then dropped, so
/// every job printed exactly once, portrait, no matter what the user chose in
/// the Mac/phone print dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PrintOptions {
    /// Clamped to 1..=999, the range `copies-supported` advertises.
    copies: u32,
    /// IPP `orientation-requested`: 3 = portrait, 4 = landscape,
    /// 5 = reverse-landscape, 6 = reverse-portrait. Anything else falls back
    /// to portrait, per RFC 8011 4.2.14.
    orientation: i32,
}

/// RFC 8011 4.2.14 `orientation-requested` values this printer honours.
const ORIENTATION_PORTRAIT: i32 = 3;
const ORIENTATION_LANDSCAPE: i32 = 4;
const ORIENTATION_REVERSE_LANDSCAPE: i32 = 5;
const ORIENTATION_REVERSE_PORTRAIT: i32 = 6;

/// One copy, portrait: exactly what a client gets when it says nothing at
/// all. Written by hand rather than derived, because `#[derive(Default)]`
/// would give `copies: 0` - a job nobody asked to print - and an
/// orientation outside the range the IPP spec defines.
impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            copies: 1,
            orientation: ORIENTATION_PORTRAIT,
        }
    }
}

impl PrintOptions {
    /// Reads the job-template group, keeping `base` for anything the client
    /// did not repeat.
    ///
    /// The merge matters for the two-step Mac flow: Create-Job carries
    /// `copies` / `orientation-requested`, and the follow-up Send-Document
    /// usually does *not* repeat them. Reading Send-Document alone would
    /// silently reset both to the defaults and print one portrait copy.
    fn from_request(attrs: &IppAttributes, base: PrintOptions) -> PrintOptions {
        let copies = req_int(attrs, "copies")
            .map(|c| u32::try_from(c).unwrap_or(1).clamp(1, 999))
            .unwrap_or(base.copies);
        let orientation = req_int(attrs, "orientation-requested")
            .filter(|o| (ORIENTATION_PORTRAIT..=ORIENTATION_REVERSE_PORTRAIT).contains(o))
            .unwrap_or(base.orientation);
        PrintOptions {
            copies,
            orientation,
        }
    }

    /// A human-readable form for job messages and logs.
    fn describe(&self) -> String {
        let orientation = match self.orientation {
            ORIENTATION_LANDSCAPE => "landscape",
            ORIENTATION_REVERSE_LANDSCAPE => "reverse-landscape",
            ORIENTATION_REVERSE_PORTRAIT => "reverse-portrait",
            _ => "portrait",
        };
        format!("{} x {orientation}", self.copies)
    }

    /// The `-print-settings` list SumatraPDF understands.
    ///
    /// * Copies: `3x` prints the document three times (SumatraPDF docs:
    ///   `-print-settings "3x"`).
    /// * Orientation: `portrait` / `landscape` rotate the contents by 90
    ///   degrees; the two *reverse* IPP values are the same orientation plus
    ///   `rotate=180`, which SumatraPDF applies on top of it.
    fn sumatra_print_settings(self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.copies > 1 {
            parts.push(format!("{}x", self.copies));
        }
        match self.orientation {
            ORIENTATION_LANDSCAPE => parts.push("landscape".into()),
            ORIENTATION_REVERSE_LANDSCAPE => {
                parts.push("landscape".into());
                parts.push("rotate=180".into());
            }
            ORIENTATION_REVERSE_PORTRAIT => {
                parts.push("portrait".into());
                parts.push("rotate=180".into());
            }
            // Portrait is the engine's own default; an explicit `portrait`
            // would only switch off SumatraPDF's auto-rotation of wide pages.
            _ => {}
        }
        parts.join(",")
    }
}

/// Arguments for the bundled printer:
///
///     SumatraPDF.exe -print-to "Printer Name" -silent -print-settings "3x,landscape" -exit-when-done "C:\path\job.pdf"
///
/// * `-print-to <name>` targets the queue the user picked in the app.
///   `-print-to-default` is used when no queue is named, which is what the
///   machine's default printer receives. Printing to the picked queue
///   directly is deliberate: this PC usually has several printers, and
///   `-print-to-default` alone would ignore the selection and silently send
///   the job somewhere else.
/// * `-silent` keeps every error dialog off the screen.
/// * `-print-settings` carries the copies count and page orientation the IPP
///   client asked for. It is only added when the job actually asks for more
///   than one portrait copy, so the plain single-copy path stays byte-for-byte
///   the command it has always been.
/// * `-exit-when-done` makes it exit once the job is spooled. Without it the
///   viewer stays open and the call never returns.
fn sidecar_args(printer: &str, path: &Path, options: PrintOptions) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let printer = printer.trim();
    if printer.is_empty() {
        args.push("-print-to-default".to_string());
    } else {
        args.push("-print-to".to_string());
        args.push(printer.to_string());
    }
    args.push("-silent".to_string());
    let settings = options.sumatra_print_settings();
    if !settings.is_empty() {
        args.push("-print-settings".to_string());
        args.push(settings);
    }
    args.push("-exit-when-done".to_string());
    args.push(path.to_string_lossy().to_string());
    args
}

#[cfg(windows)]
fn sidecar_default_args(path: &Path, options: PrintOptions) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    args.push("-print-to-default".to_string());
    args.push("-silent".to_string());
    let settings = options.sumatra_print_settings();
    if !settings.is_empty() {
        args.push("-print-settings".to_string());
        args.push(settings);
    }
    args.push("-exit-when-done".to_string());
    args.push(path.to_string_lossy().to_string());
    args
}

#[cfg(windows)]
fn set_windows_default_printer(printer: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let printer_ps = printer.replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference = 'SilentlyContinue'; \
         $p = Get-CimInstance -ClassName Win32_Printer -Filter \"Name='{printer_ps}'\"; \
         if ($null -ne $p) {{ Invoke-CimMethod -InputObject $p -MethodName SetDefaultPrinter | Out-Null }}"
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

#[cfg(windows)]
fn print_image_windows(
    printer: &str,
    path: &Path,
    title: &str,
    options: PrintOptions,
) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let file = path.to_string_lossy().replace('\'', "''");
    let printer_ps = printer.replace('\'', "''");
    let copies = options.copies.max(1);
    let landscape = match options.orientation {
        ORIENTATION_LANDSCAPE | ORIENTATION_REVERSE_LANDSCAPE => "$true",
        _ => "$false",
    };

    let script = format!(
        r#"$ErrorActionPreference = 'Stop';
Add-Type -AssemblyName System.Drawing;
$filePath = '{file}';
$printer = '{printer_ps}';
if (-not (Test-Path $filePath)) {{ throw 'File not found' }};
$doc = New-Object System.Drawing.Printing.PrintDocument;
$doc.PrinterSettings.PrinterName = $printer;
if (-not $doc.PrinterSettings.IsValid) {{ throw "Printer invalid or not found: $printer" }};
$doc.PrinterSettings.Copies = {copies};
$doc.DefaultPageSettings.Landscape = {landscape};
$doc.DocumentName = '{title}';
$img = [System.Drawing.Image]::FromFile($filePath);
$doc.add_PrintPage({{
    param($s, $e)
    $page = $e.PageBounds;
    $imgW = $img.Width;
    $imgH = $img.Height;
    $scaleX = $page.Width / $imgW;
    $scaleY = $page.Height / $imgH;
    $scale = [Math]::Min($scaleX, $scaleY);
    $destW = [int]($imgW * $scale);
    $destH = [int]($imgH * $scale);
    $destX = $page.Left + [int](($page.Width - $destW) / 2);
    $destY = $page.Top + [int](($page.Height - $destH) / 2);
    $e.Graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic;
    $e.Graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality;
    $e.Graphics.DrawImage($img, $destX, $destY, $destW, $destH);
    $e.HasMorePages = $false;
}});
$doc.Print();
$img.Dispose();
$doc.Dispose();"#
    );

    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match out {
        Ok(o) if o.status.success() => {
            logln!("Image '{title}' successfully printed on '{printer}'");
            Ok(String::new())
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                stderr
            };
            logln!("Image print on '{printer}' failed: {detail}");
            Err(detail)
        }
        Err(e) => Err(format!("Could not execute PowerShell image print: {e}")),
    }
}

/// Where the sidecar may live: next to the running exe (build output and
/// installed builds), in the Tauri resource folder, and in the project tree
/// for `tauri dev` / `cargo run`.
fn sidecar_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(SIDECAR_EXE));
            candidates.push(dir.join("bin").join(SIDECAR_EXE));
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join(SIDECAR_EXE));
        candidates.push(res.join("bin").join(SIDECAR_EXE));
    }
    candidates.push(PathBuf::from("src-tauri").join("bin").join(SIDECAR_EXE));
    candidates.push(PathBuf::from("bin").join(SIDECAR_EXE));
    candidates.into_iter().find(|p| p.is_file())
}

/// Drops SumatraPDF's `ParseFlags:` chatter and blank lines from a captured
/// sidecar stream, keeping only what can actually explain a failure.
fn sidecar_failure_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("ParseFlags:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints through the sidecar with no window, no viewer and no dialog.
///
/// Returns `None` only when there is no sidecar to run, which tells the caller
/// to fall back to the platform printer path instead of failing the job.
/// Once the sidecar is running, a failure is reported honestly rather than
/// falling through - the tool may already have spooled the job, and printing
/// twice is worse than failing once.
#[cfg(windows)]
fn print_with_sidecar(
    app: &tauri::AppHandle,
    printer: &str,
    path: &Path,
    title: &str,
    options: PrintOptions,
) -> Option<Result<(), String>> {
    let exe = sidecar_path(app)?;
    // `run_hidden` supplies both halves of "silent": no console window, and a
    // bounded wait so a stuck spooler cannot wedge the print pipeline.
    let mut out = run_hidden(
        &exe,
        &sidecar_args(printer, path, options),
        SIDECAR_TIMEOUT,
    );

    // If -print-to failed (e.g. queues with parentheses like 'HP LaserJet (Shop)'),
    // retry with -print-to-default because the queue was set as Windows default.
    if let Some(ref o) = out {
        if !o.status.success() {
            logln!("SumatraPDF -print-to failed ({:?}). Retrying with -print-to-default...", o.status);
            out = run_hidden(
                &exe,
                &sidecar_default_args(path, options),
                SIDECAR_TIMEOUT,
            );
        }
    }

    match out {
        Some(o) if o.status.success() => {
            logln!(
                "Silently printed '{title}' ({}) on '{printer}' via {}",
                options.describe(),
                exe.display()
            );
            Some(Ok(()))
        }
        Some(o) => {
            // SumatraPDF 3.6 writes `ParseFlags: argName: ..., arg: N` to stdout
            // for every flag it *accepts* - on a successful run too - so that
            // output is debug chatter, not a reason. Reporting it verbatim (which
            // is what falling through to stdout did) meant every failed print was
            // blamed on `-print-to` and the actual cause, the printer, was lost.
            let reason = sidecar_failure_lines(&String::from_utf8_lossy(&o.stderr));
            let reason = if reason.is_empty() {
                sidecar_failure_lines(&String::from_utf8_lossy(&o.stdout))
            } else {
                reason
            };
            // The queue and the exit status are the two facts that identify the
            // failure; the printer's own complaint, if it produced any, follows.
            let mut detail = format!(
                "printing on '{printer}' failed ({} exited with {})",
                exe.display(),
                o.status
            );
            if !reason.is_empty() {
                detail.push_str(&format!("\n{reason}"));
            }
            logln!("Silent print failed: {detail}");
            Some(Err(detail))
        }
        None => Some(Err(format!(
            "{} did not finish within {}s",
            exe.display(),
            SIDECAR_TIMEOUT.as_secs()
        ))),
    }
}

/// Sends `path` to `printer` through whatever the OS ships with, honouring
/// the copies count and orientation the IPP client asked for.
///
/// Returns the CUPS job id to poll (`""` when there is nothing to poll).
#[cfg(windows)]
fn send_to_printer(
    app: &tauri::AppHandle,
    printer: &str,
    path: &Path,
    title: &str,
    options: PrintOptions,
) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_image = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "bmp" | "webp" | "gif");

    if is_image {
        logln!("Printing image '{}' to '{printer}' via Windows PrintDocument", path.display());
        return print_image_windows(printer, path, title, options);
    }

    // Set Windows default printer so both -print-to-default and shell verb target the right device
    set_windows_default_printer(printer);

    // Silent sidecar first - it never opens a PDF viewer.
    if let Some(result) = print_with_sidecar(app, printer, path, title, options) {
        if result.is_ok() {
            return Ok(String::new());
        }
        logln!("Silent sidecar failed: {:?}. Falling back to shell print.", result);
    }

    logln!("Silent sidecar unavailable - falling back to Start-Process -Verb Print");
    // The shell verb has no copies or orientation knob at all, so the only
    // way to honour `copies` here is to submit the document once per copy.
    for copy in 1..=options.copies.max(1) {
        if copy > 1 {
            // One spool at a time: each `Start-Process -Verb Print` returns
            // once the job is in the queue, so the copies land in order.
            std::thread::sleep(Duration::from_millis(250));
        }
        print_with_shell(printer, path, title)?;
    }
    Ok(String::new())
}

/// Windows printing: hand the file to the shell's `print` verb through
/// PowerShell. This is the Windows equivalent of `lp` - it ships with every
/// install, so the app stays self-contained (no CUPS, no Bonjour, no extra
/// tooling).
///
/// `Start-Process -Verb Print` always prints on the *default* printer, so the
/// queue the user picked is made the default first. That step is best effort:
/// if it fails the job still goes out on the current default instead of
/// erroring, and nothing here is allowed to take the app down.
#[cfg(windows)]
fn print_with_shell(printer: &str, path: &Path, title: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console window flashes up

    let file = path.to_string_lossy().replace('\'', "''");
    let printer_ps = printer.replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
         try {{ $p = Get-CimInstance -ClassName Win32_Printer -Filter \"Name='{printer_ps}'\"; \
                if ($null -ne $p) {{ Invoke-CimMethod -InputObject $p -MethodName SetDefaultPrinter | Out-Null }} \
         }} catch {{ }}; \
         Start-Process -FilePath '{file}' -Verb Print"
    );

    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match out {
        Ok(o) if o.status.success() => {
            logln!("Sent '{title}' to '{printer}' via Start-Process -Verb Print");
            Ok(())
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let detail = if stderr.trim().is_empty() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            if detail.is_empty() {
                Err(format!(
                    "Windows could not print '{}' to {printer}",
                    path.display()
                ))
            } else {
                Err(detail)
            }
        }
        Err(e) => Err(format!("Could not run PowerShell to print: {e}")),
    }
}

#[cfg(not(windows))]
fn send_to_printer(
    _app: &tauri::AppHandle,
    printer: &str,
    path: &Path,
    title: &str,
    options: PrintOptions,
) -> Result<String, String> {
    run_lp(printer, path, title, options)
}

#[cfg(not(windows))]
fn run_lp(printer: &str, path: &Path, title: &str, options: PrintOptions) -> Result<String, String> {
    let mut cmd = Command::new("lp");
    cmd.arg("-d").arg(printer).arg("-t").arg(title);
    // CUPS honours both natively: `-n` is the copies count, `-o landscape`
    // the page orientation (the reverse variants map onto landscape plus the
    // CUPS `orientation-requested` enum).
    if options.copies > 1 {
        cmd.arg("-n").arg(options.copies.to_string());
    }
    match options.orientation {
        ORIENTATION_LANDSCAPE => {
            cmd.arg("-o").arg("landscape");
        }
        ORIENTATION_REVERSE_LANDSCAPE => {
            cmd.arg("-o").arg("orientation-requested=5");
        }
        ORIENTATION_REVERSE_PORTRAIT => {
            cmd.arg("-o").arg("orientation-requested=6");
        }
        _ => {}
    }
    let out = cmd
        .arg(path)
        .output()
        .map_err(|e| format!("Could not run lp: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() {
            let so = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if so.is_empty() {
                "lp rejected the job".to_string()
            } else {
                so
            }
        } else {
            err
        };
        return Err(msg);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let cups_id = stdout
        .split("request id is ")
        .nth(1)
        .map(|s| s.split(" (").next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    Ok(cups_id)
}

fn wait_for_cups_job(cups_id: &str) -> bool {
    if cups_id.is_empty() {
        return true;
    }
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        match Command::new("lpstat").arg("-o").output() {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                if !text.lines().any(|l| l.starts_with(cups_id)) {
                    return true;
                }
            }
            _ => return true,
        }
    }
    false
}

fn emit_job(state: &IppState, info: &JobInfo, event: &str) {
    if let Some(app) = &state.app {
        let _ = app.emit(event, info.clone());
    }
}

fn emit_job_by_id(app: &tauri::AppHandle, jobs: &Jobs, id: u32, event: &str) {
    if let Some(info) = jobs.get_info(id) {
        let _ = app.emit(event, info);
    }
}

#[tauri::command]
async fn print_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    job_id: u32,
    printer: String,
) -> Result<String, String> {
    if printer.trim().is_empty() {
        return Err("No printer selected".into());
    }

    // Printing into a queue that belongs to this process is a loop: the job
    // comes back out of the spooler as an IPP request and is printed again,
    // endlessly. The device URI catches it on CUPS; on Windows that helper
    // shells out to `lpstat`, which does not exist there, so the queue's name
    // is checked too - it is the one identity `Add to Windows` leaves behind.
    let device_name = state.printer_name.lock().unwrap().clone();
    if is_our_own_queue(&printer, &device_name)
        || device_uri_of(&printer).map(|u| is_self_uri(&u)).unwrap_or(false)
    {
        return Err(
            "'Nearby Print' on this PC points back at this app. Choose a real printer.".into(),
        );
    }

    let (path, title, options) = {
        let mut map = state.jobs.map.lock().unwrap();
        let job = map.get_mut(&job_id).ok_or_else(|| "Unknown job".to_string())?;
        if job.info.bytes == 0 || job.path.is_none() {
            return Err("No document received for this job yet".into());
        }
        if job.info.status == "completed" || job.info.status == "canceled" {
            return Err("Job already finished".into());
        }
        job.info.status = "printing".into();
        job.info.message = format!("Sending ({}) to {printer}…", job.options.describe());
        job.info.printer = printer.clone();
        job.ipp_state = JOB_PROCESSING;
        let info = job.info.clone();
        (job.path.clone().unwrap(), info.name.clone(), job.options)
    };

    emit_job_by_id(&app, &state.jobs, job_id, "job_updated");

    let app_c = app.clone();
    let printer_c = printer.clone();
    let result =
        tokio::task::spawn_blocking(move || send_to_printer(&app_c, &printer_c, &path, &title, options))
            .await
            .map_err(|e| e.to_string())?;

    match result {
        Ok(cups_id) => {
            let finished = tokio::task::spawn_blocking(move || wait_for_cups_job(&cups_id))
                .await
                .unwrap_or(true);

            let info = state.jobs.update(job_id, |j| {
                if finished {
                    j.info.status = "completed".into();
                    j.info.message = format!("Printed on {printer}");
                    j.ipp_state = JOB_COMPLETED;
                    if let Some(p) = j.path.take() {
                        let _ = std::fs::remove_file(p);
                    }
                } else {
                    j.info.status = "printing".into();
                    j.info.message = format!("Queued on {printer}");
                }
            });
            if let Some(info) = info {
                let _ = app.emit("job_updated", info);
            }
            Ok(if finished {
                format!("Job sent to {printer}.")
            } else {
                format!("Job queued on {printer}.")
            })
        }
        Err(e) => {
            let info = state.jobs.update(job_id, |j| {
                j.info.status = "error".into();
                j.info.message = e.clone();
                j.ipp_state = JOB_ABORTED;
            });
            if let Some(info) = info {
                let _ = app.emit("job_updated", info);
            }
            Err(e)
        }
    }
}

#[tauri::command]
fn cancel_job(app: tauri::AppHandle, state: tauri::State<'_, AppState>, job_id: u32) -> Result<(), String> {
    if let Some(path) = state.jobs.take_path(job_id) {
        let _ = std::fs::remove_file(path);
    }
    let info = state.jobs.update(job_id, |j| {
        j.info.status = "canceled".into();
        j.info.message = "Rejected in Nearby Print".into();
        j.ipp_state = JOB_CANCELED;
    });
    if let Some(info) = info {
        let _ = app.emit("job_updated", info);
    }
    Ok(())
}

#[tauri::command]
async fn submit_cloud_job(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    customer: String,
    filename: String,
    data: Vec<u8>,
    content_type: Option<String>,
    copies: Option<u32>,
    color_mode: Option<String>,
) -> Result<u32, String> {
    if data.is_empty() {
        return Err("Empty document data received".into());
    }

    let fmt = content_type.unwrap_or_else(|| "application/pdf".into());
    let (path, _, ext, size) = save_document_to_temp(&data, &fmt);
    let job_id = next_job_id();
    let display_name = if filename.trim().is_empty() {
        format!("cloud_doc_{}.{}", job_id, ext)
    } else {
        filename
    };
    let copies_num = copies.unwrap_or(1).clamp(1, 99);
    let color = color_mode.unwrap_or_else(|| "bw".into());
    let path_str = path.to_string_lossy().to_string();

    let info = JobInfo {
        id: job_id,
        name: display_name,
        device: format!("Cloud: {}", customer),
        user: customer.clone(),
        pages: 1,
        bytes: size,
        format: ext,
        status: "pending".into(),
        message: format!(
            "Cloud Order from {} ({} cop{}, {})",
            customer,
            copies_num,
            if copies_num > 1 { "ies" } else { "y" },
            if color == "color" { "Color" } else { "B&W" }
        ),
        printer: String::new(),
        file_path: Some(path_str),
        channel: Some("cloud".into()),
        customer: Some(customer),
        color_mode: Some(color),
        is_card: Some(false),
    };

    {
        let mut map = state.jobs.map.lock().unwrap();
        map.insert(
            job_id,
            Job {
                info: info.clone(),
                path: Some(path),
                options: PrintOptions {
                    copies: copies_num,
                    orientation: ORIENTATION_PORTRAIT,
                },
                ipp_state: JOB_PENDING,
            },
        );
    }

    emit_job_by_id(&app, &state.jobs, job_id, "job_created");
    Ok(job_id)
}

// ---------------------------------------------------------------------------
// IPP helpers
// ---------------------------------------------------------------------------

fn req_attr<'a>(attrs: &'a IppAttributes, name: &str) -> Option<&'a IppValue> {
    attrs
        .groups()
        .iter()
        .find_map(|g| g.attributes().get(name).map(|a| a.value()))
}

fn value_to_string(v: &IppValue) -> Option<String> {
    match v {
        IppValue::NameWithoutLanguage(s) => Some(s.to_string()),
        IppValue::TextWithoutLanguage(s) => Some(s.to_string()),
        IppValue::Keyword(s) => Some(s.to_string()),
        IppValue::MimeMediaType(s) => Some(s.to_string()),
        IppValue::Uri(s) => Some(s.to_string()),
        IppValue::UriScheme(s) => Some(s.to_string()),
        IppValue::Charset(s) => Some(s.to_string()),
        IppValue::NaturalLanguage(s) => Some(s.to_string()),
        IppValue::OctetString(s) => Some(s.to_string()),
        IppValue::NameWithLanguage { name, .. } => Some(name.to_string()),
        IppValue::TextWithLanguage { text, .. } => Some(text.to_string()),
        _ => None,
    }
}

fn req_str(attrs: &IppAttributes, name: &str) -> Option<String> {
    req_attr(attrs, name).and_then(value_to_string)
}

fn req_int(attrs: &IppAttributes, name: &str) -> Option<i32> {
    match req_attr(attrs, name)? {
        IppValue::Integer(i) | IppValue::Enum(i) => Some(*i),
        _ => None,
    }
}

/// `last-document` and friends are booleans in the IPP encoding. Kept beside
/// the other RFC 8011 readers so the next caller does not re-match `IppValue`
/// by hand; nothing in the current request paths needs it yet.
#[allow(dead_code)]
fn req_bool(attrs: &IppAttributes, name: &str) -> Option<bool> {
    match req_attr(attrs, name)? {
        IppValue::Boolean(v) => Some(*v),
        _ => None,
    }
}

fn req_job_id(attrs: &IppAttributes) -> Option<u32> {
    if let Some(v) = req_int(attrs, "job-id") {
        return Some(v as u32);
    }
    let uri = req_str(attrs, "job-uri")?;
    let tail = uri.rsplit('/').next()?;
    tail.trim().parse().ok()
}

fn add_to_group(group: &mut IppAttributeGroup, name: &str, value: IppValue) {
    if let Ok(n) = IppName::try_from(name) {
        group.attributes_mut().insert(n.clone(), IppAttribute::new(n, value));
    }
}

/// The job attributes group every job-bearing response must carry:
/// `job-id`, `job-uri`, `job-state` and `job-state-reasons`.
///
/// RFC 8011 4.3.3 (Print-Job) and 4.3.4 (Send-Document) make the first three
/// **mandatory** in a success response. A Windows spooler will not clear a job
/// from its local queue until it has an id it can address, a state it can
/// interpret and a URI it can poll - answer with a bare status code and it
/// treats the submission as refused and flips the queue to "Printing, Error"
/// straight away, which is exactly the failure this function exists to prevent.
///
/// The URI must point back at *this* server: `ipp://localhost/jobs/N` resolves
/// to the client's own loopback, and a client that follows it gets a refused
/// connection and reports the job as failed.
fn add_job_group(resp: &mut IppRequestResponse, state: &IppState, job_id: u32, job_state: i32) {
    resp.attributes_mut()
        .groups_mut()
        .push(IppAttributeGroup::new(DelimiterTag::JobAttributes));
    let job_group = resp.attributes_mut().groups_mut().last_mut().unwrap();
    add_to_group(job_group, "job-id", IppValue::Integer(job_id as i32));
    add_to_group(job_group, "job-state", IppValue::Enum(job_state));
    add_to_group(job_group, "job-state-reasons", IppValue::Keyword("none".try_into().unwrap()));
    let host = if state.local_ip.is_empty() {
        "localhost".to_string()
    } else {
        state.local_ip.clone()
    };
    add_to_group(
        job_group,
        "job-uri",
        IppValue::Uri(format!("ipp://{host}:{}/jobs/{job_id}", state.port).try_into().unwrap()),
    );
}

/// Maps this process's own `Job::ipp_state` bookkeeping onto the RFC 8011 4.3.7
/// `job-state` enumeration that actually goes on the wire.
///
/// The two numberings are deliberately different - `JOB_PENDING` is 1 while
/// `pending` is 3, `JOB_COMPLETED` is 7 while `completed` is 9 - so sending
/// `ipp_state` verbatim hands a client a value its spec has no meaning for.
/// A print service reading an unknown state treats the job as still running
/// and keeps polling it: exactly the loop that turns one submission into a
/// few hundred Get-Jobs requests.
fn rfc_job_state(internal: u16) -> i32 {
    match internal {
        JOB_PROCESSING => RFC_JOB_STATE_PROCESSING,
        JOB_CANCELED => RFC_JOB_STATE_CANCELED,
        JOB_ABORTED => RFC_JOB_STATE_ABORTED,
        JOB_COMPLETED => RFC_JOB_STATE_COMPLETED,
        _ => RFC_JOB_STATE_PENDING,
    }
}

/// RFC 8011 4.3.7: 7 (canceled), 8 (aborted) and 9 (completed) are terminal.
/// A client whose job never reaches one of them asks about it indefinitely.
fn is_terminal_job_state(rfc_state: i32) -> bool {
    matches!(rfc_state, RFC_JOB_STATE_CANCELED | RFC_JOB_STATE_ABORTED | RFC_JOB_STATE_COMPLETED)
}

/// `job-name` / `job-originating-user-name` on the job group `add_job_group`
/// just pushed. `job-name` is what lets a client pick *its* job out of the
/// list Get-Jobs returns, so without it a multi-job response is unattributable.
fn add_job_name(resp: &mut IppRequestResponse, name: &str, user: &str) {
    let Some(group) = resp.attributes_mut().groups_mut().last_mut() else {
        return;
    };
    add_to_group(
        group,
        "job-name",
        IppValue::NameWithoutLanguage(name.try_into().unwrap()),
    );
    add_to_group(
        group,
        "job-originating-user-name",
        IppValue::NameWithoutLanguage(user.try_into().unwrap()),
    );
}

// ---------------------------------------------------------------------------
// mDNS / discovery
// ---------------------------------------------------------------------------

/// Mandatory AirPrint / IPP TXT records for `_ipp._tcp` (PWG 5100.14 §6 — the
/// same set Mopria, AirPrint and the Windows spooler parse).
///
/// A spooler only turns an mDNS answer into a printer when the TXT record
/// declares *what* it is: `rp` (the IPP resource path), `ty` (description),
/// `note`, `pdl` (formats it accepts) plus the `Color`/`Duplex` capabilities.
/// Without them the service is visible in a packet capture but never offered
/// in auto-discovery. Keys are compared case-insensitively (RFC 6763 §6.4).
///
/// `txtvers`, `qtotal`, `rp`, `ty`, `note`, `pdl` and `URF` are the fields
/// macOS/iOS check before they list a discovered service as an *AirPrint*
/// printer; a record missing `pdl` or `URF` is browsed but never offered in
/// the Add Printer dialog.
fn txt_properties() -> [(&'static str, &'static str); 9] {
    [
        ("txtvers", "1"),
        ("qtotal", "1"),
        ("rp", "ipp/print"),
        ("ty", "NearbyPrint"),
        ("note", "Local Printer"),
        // Same contract as `document-format-supported`, plus `image/urf` in
        // the spelling AirPrint clients look for.
        ("pdl", "application/pdf,image/urf"),
        ("URF", "V1.4,W8,SRGB24,DM1"),
        ("Color", "T"),
        ("Duplex", "F"),
    ]
}

/// Builds the TXT property map that is attached to the `ServiceInfo`.
fn txt_property_map() -> HashMap<String, String> {
    txt_properties()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The SRV target (and A-record name) this machine announces on mDNS.
///
/// Unique per PC, and that is the whole point: when every NearbyPrint used the
/// same `NearbyPrint.local.` target, an mDNS resolver merged *all* their A
/// records under that one name, so each discovered instance resolved to
/// whichever addresses the cache happened to hold - the host list then showed
/// every entry pointing at the same PC. A per-machine name keeps each
/// instance's SRV target resolvable to exactly one machine.
fn mdns_server_hostname() -> String {
    // COMPUTERNAME on Windows, HOSTNAME elsewhere. Either may contain
    // characters a DNS label rejects (spaces, underscores), so they are
    // folded to `[a-z0-9-]` before use.
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let label: String = raw
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() {
        "nearbyprint-host.local.".into()
    } else {
        format!("{label}.local.")
    }
}

/// Registers `_ipp._tcp.local.` (plus the AirPrint `_universal` subtype) via
/// the pure-Rust `mdns-sd` daemon.
///
/// Nothing external is spawned: the daemon binds `0.0.0.0:5353`
/// (SO_REUSEADDR) on every multicast-capable interface of this machine and
/// answers mDNS queries itself, so Apple Bonjour / `dns-sd` is not required.
///
/// The registration uses [`AIRPRINT_SERVICE_TYPE`]: `mdns-sd` splits it on
/// `._sub.` and announces a PTR for *both* the parent `_ipp._tcp.local.` and
/// the `_universal` subtype, under one instance fullname. That single
/// registration is what makes the same printer discoverable by Windows and
/// Android (parent type) *and* by macOS / iOS AirPrint (subtype) - the Mac
/// browses only the subtype and ignored us entirely before.
fn start_mdns_sd(ip: &str, printer_name: &str) -> Result<ServiceDaemon, String> {
    let mdns = ServiceDaemon::new().map_err(|e| format!("mDNS daemon failed to start: {e}"))?;

    // The IPP server address first, then every other routable LAN address, so
    // a client on a different subnet still resolves a usable IPv4 for us.
    let mut addresses: Vec<String> = Vec::new();
    if !ip.is_empty() {
        addresses.push(ip.to_string());
    }
    for extra in lan_ipv4s() {
        if !addresses.contains(&extra) {
            addresses.push(extra);
        }
    }
    if addresses.is_empty() {
        return Err("No LAN IPv4 address available to broadcast on".into());
    }

    // Build the TXT property map and attach it to the ServiceInfo *before*
    // register(), so the daemon announces PTR + SRV + TXT + A together in
    // every response and every cache-flush repeat.
    let properties = txt_property_map();

    let info = ServiceInfo::new(
        // The subtype form, so one registration answers both the plain IPP
        // browse (Windows / Android / our own host discovery) and the
        // `_universal` browse macOS and iOS use for AirPrint. The instance
        // fullname stays `<name>._ipp._tcp.local.`, which is what
        // `unregister_mdns` and host discovery already key on.
        AIRPRINT_SERVICE_TYPE,
        printer_name,
        // Per-machine SRV target - see `mdns_server_hostname` for why a
        // shared name collapses every host into one address.
        &mdns_server_hostname(),
        addresses.join(","),
        IPP_PORT,
        Some(properties),
    )
    .map_err(|e| format!("invalid mDNS service info: {e}"))?
    // Keep the announced addresses in sync when interfaces appear or vanish
    // or the machine picks up a new DHCP lease - this is what keeps Mac and
    // mobile printing after an IP change without the printer being re-added.
    .enable_addr_auto();

    mdns.register(info)
        .map_err(|e| format!("mDNS registration failed: {e}"))?;
    logln!(
        "mDNS (pure Rust mdns-sd) registered '{printer_name}' on {MDNS_SERVICE_TYPE} + {AIRPRINT_SERVICE_TYPE} at {}:{IPP_PORT}",
        addresses.join(",")
    );
    logln!(
        "mDNS TXT: {}",
        txt_properties()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    Ok(mdns)
}

/// True while the background `mdns-sd` daemon answers to us.
///
/// This probes the daemon thread instead of shelling out to `dns-sd`, which
/// does not exist on a stock Windows install.
fn mdns_daemon_is_running(mdns: &ServiceDaemon) -> bool {
    match mdns.status() {
        Ok(status) => matches!(
            status.recv_timeout(Duration::from_millis(500)),
            Ok(DaemonStatus::Running)
        ),
        // The command channel is gone: the daemon thread already exited.
        Err(_) => false,
    }
}

fn unregister_mdns(mdns: &ServiceDaemon, printer_name: &str) {
    for full in [
        format!("{printer_name}._ipp._tcp.local.").as_str(),
        format!("{printer_name}._ipps._tcp.local.").as_str(),
    ] {
        let _ = mdns.unregister(full);
    }
}

// ---------------------------------------------------------------------------
// Host discovery (client role)
// ---------------------------------------------------------------------------

/// One NearbyPrint host found on the LAN, in the shape the UI needs.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct DiscoveredHost {
    /// The instance name the host registered under - the same string its own
    /// header shows, so both ends of the pairing read alike.
    name: String,
    /// The first IPv4 address the host announced.
    ip: String,
    /// The IPP port it announced. Our hosts say [`IPP_PORT`].
    port: u16,
    /// What `set_relay_host` accepts: a bare address, or `address:port` when
    /// the host is not on the default port.
    host: String,
    /// Where that host's network IPP server answers.
    url: String,
}

/// The instance name out of `<instance>._ipp._tcp.local.`, which is all
/// `ServiceInfo::get_fullname` carries.
///
/// `ServiceResolved` hands over the name with its trailing dot and
/// `ServiceRemoved` may not have it, so both spellings have to reduce to the
/// same string - otherwise a host could be added under one name and never
/// match on removal.
fn mdns_instance_name(fullname: &str) -> String {
    let without_type = fullname
        .strip_suffix(MDNS_SERVICE_TYPE)
        .or_else(|| fullname.strip_suffix(MDNS_SERVICE_TYPE.trim_end_matches('.')))
        .unwrap_or(fullname);
    without_type.trim_end_matches('.').to_string()
}

/// Turns a resolved mDNS record into a relay target.
///
/// Returns `None` for anything that cannot be printed to: a record with no
/// IPv4 address, or one whose every address belongs to *this* machine - which
/// is what our own advertisement looks like when we are broadcasting. Offering
/// that back in the list would make "pick a host" a choice between relaying
/// to ourselves and relaying to ourselves.
fn discovered_host_from(info: &ServiceInfo) -> Option<DiscoveredHost> {
    let own = lan_ipv4s();
    let mut addresses: Vec<String> = info
        .get_addresses()
        .iter()
        .filter_map(|addr| match addr {
            IpAddr::V4(v4) => Some(v4.to_string()),
            IpAddr::V6(_) => None,
        })
        .filter(|ip| !own.contains(ip))
        .collect();
    // Sorted so a multi-homed host always shows up with the same address
    // instead of whatever the hash set felt like that run.
    addresses.sort();
    let ip = addresses.into_iter().next()?;
    let port = info.get_port();
    let host = if port == IPP_PORT {
        ip.clone()
    } else {
        format!("{ip}:{port}")
    };
    Some(DiscoveredHost {
        name: mdns_instance_name(info.get_fullname()),
        url: format!("ipp://{ip}:{port}/ipp/print"),
        ip,
        port,
        host,
    })
}

fn discovered_hosts_snapshot(state: &tauri::State<'_, AppState>) -> Vec<DiscoveredHost> {
    state
        .discovered_hosts
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default()
}

/// Starts listening for other NearbyPrint hosts on `_ipp._tcp.local.`
///
/// The mirror image of `start_mdns_sd`: the very record a phone browses is
/// browsed here, so "which PC has the printer" is answered by the network
/// instead of by a text field the user has to fill in. Idempotent - the
/// frontend calls it on every load to heal a start that failed at launch.
#[tauri::command]
fn start_discovery(app: tauri::AppHandle) -> Result<(), String> {
    let rx = {
        let state = app.state::<AppState>();
        // One daemon, held across the whole check-then-set so two calls can
        // never both pass the test and end up with two browses nobody stops.
        let mut slot = state
            .discovery_daemon
            .lock()
            .map_err(|_| "mDNS discovery state is locked".to_string())?;
        if slot.is_some() {
            return Ok(());
        }
        let daemon =
            ServiceDaemon::new().map_err(|e| format!("mDNS daemon failed to start: {e}"))?;
        let rx = daemon
            .browse(MDNS_SERVICE_TYPE)
            .map_err(|e| format!("mDNS browse for {MDNS_SERVICE_TYPE} failed: {e}"))?;
        *slot = Some(daemon);
        rx
    };

    let hosts = app.state::<AppState>().discovered_hosts.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            let changed = match event {
                ServiceEvent::ServiceResolved(info) => match discovered_host_from(&info) {
                    Some(host) => {
                        let mut list = match hosts.lock() {
                            Ok(list) => list,
                            // The app is shutting down: stop rather than
                            // keep poking a poisoned lock.
                            Err(_) => break,
                        };
                        // A position lookup rather than `iter_mut().find`:
                        // the slot borrow would otherwise outlive the `None`
                        // arm, which is the one that has to push.
                        match list.iter().position(|seen| seen.name == host.name) {
                            Some(index) => {
                                if list[index] == host {
                                    false
                                } else {
                                    // A host that re-announces with a new
                                    // address is updated in place, not
                                    // duplicated.
                                    list[index] = host;
                                    true
                                }
                            }
                            None => {
                                list.push(host);
                                list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                                true
                            }
                        }
                    }
                    // Our own record, or one with no usable address: nothing
                    // for the user to pick.
                    None => false,
                },
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    let name = mdns_instance_name(&fullname);
                    let mut list = match hosts.lock() {
                        Ok(list) => list,
                        Err(_) => break,
                    };
                    let before = list.len();
                    list.retain(|seen| seen.name != name);
                    list.len() != before
                }
                _ => false,
            };

            if changed {
                let snapshot = match hosts.lock() {
                    Ok(list) => list.clone(),
                    Err(_) => break,
                };
                let _ = handle.emit("discovered-hosts", &snapshot);
            }
        }
    });
    Ok(())
}

/// The hosts found so far, for the first paint before any event arrives.
#[tauri::command]
fn get_discovered_hosts(state: tauri::State<'_, AppState>) -> Vec<DiscoveredHost> {
    discovered_hosts_snapshot(&state)
}

/// Starts the search over from an empty list.
///
/// A host that vanished with its network only reports `ServiceRemoved` while
/// its browse is alive, so re-browsing is the one way to drop an entry whose
/// daemon has been gone for hours - which is what this button is for.
#[tauri::command]
fn refresh_discovery(app: tauri::AppHandle) -> Result<Vec<DiscoveredHost>, String> {
    // Stop first, then clear: the old reader thread exits as soon as the
    // daemon drops its event senders, and clearing before that would let a
    // late event put a stale entry straight back.
    {
        let state = app.state::<AppState>();
        if let Ok(mut slot) = state.discovery_daemon.lock() {
            if let Some(daemon) = slot.take() {
                let _ = daemon.stop_browse(MDNS_SERVICE_TYPE);
                let _ = daemon.shutdown();
            }
        }
        if let Ok(mut list) = state.discovered_hosts.lock() {
            if !list.is_empty() {
                list.clear();
            }
        };
    }
    let _ = app.emit("discovered-hosts", &Vec::<DiscoveredHost>::new());
    start_discovery(app.clone())?;
    Ok(get_discovered_hosts(app.state::<AppState>()))
}

// ---------------------------------------------------------------------------
// Broadcast control
// ---------------------------------------------------------------------------

fn stop_broadcast(state: &tauri::State<'_, AppState>) {
    let name = state.printer_name.lock().unwrap().clone();
    if let Ok(mut mdns_opt) = state.mdns.lock() {
        if let Some(mdns) = mdns_opt.take() {
            unregister_mdns(&mdns, &name);
            let _ = mdns.shutdown();
        }
    }
    if let Ok(mut shutdown) = state.server_shutdown.lock() {
        if let Some(tx) = shutdown.take() {
            let _ = tx.try_send(());
        }
    }
    if let Ok(mut url) = state.server_url.lock() {
        url.clear();
    }
}

/// Small bits of state that have to outlive a restart, kept in the app's own
/// config directory rather than the registry: the device uuid and the last
/// broadcast setting.
const PRINTER_UUID_FILE: &str = "printer-uuid";
const BROADCAST_PREF_FILE: &str = "broadcast";
/// The printer name the user last chose. Only written when they actually
/// change it, so the hostname-derived default stays a default until then.
const PRINTER_NAME_FILE: &str = "printer-name";
/// The address this PC forwards received documents to. Empty or missing means
/// this PC is a host: it prints whatever it receives itself.
const RELAY_HOST_FILE: &str = "relay-host";
/// The local queue relayed jobs print on. Only written when the user picks
/// one, so an untouched install keeps printing to the Windows default.
const SELECTED_PRINTER_FILE: &str = "selected-printer";

/// The one route a relay client posts documents to. Kept as a constant so the
/// client and the receiver cannot drift apart into two different paths.
const RELAY_PATH: &str = "/api/relay-print";

fn config_file(app: &tauri::AppHandle, name: &str) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(name))
}

fn read_config(app: &tauri::AppHandle, name: &str) -> Option<String> {
    let path = config_file(app, name)?;
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn write_config(app: &tauri::AppHandle, name: &str, contents: &str) {
    if let Some(path) = config_file(app, name) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
    }
}

/// The printer's `printer-uuid` is a *device* identity, not a session one.
///
/// Windows records it when it installs the queue and matches on it afterwards,
/// so minting a fresh uuid on every broadcast is what leaves an already-installed
/// Windows queue pointing at a device that no longer identifies itself the same
/// way - it shows up as a queue stuck in Error, or as a second "(Copy 1)" queue
/// on the next run. Generate it once and reuse it for the life of the machine.
///
/// A file that exists but cannot be read is reported as an error rather than
/// silently replaced: one transient read failure would otherwise throw the
/// identity away and strand every queue already installed against it. Losing
/// a broadcast attempt is recoverable by clicking again; losing the identity
/// is not.
fn load_or_create_printer_uuid(app: &tauri::AppHandle) -> Result<String, String> {
    if let Some(path) = config_file(app, PRINTER_UUID_FILE) {
        if path.exists() {
            let saved = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "Cannot read the saved printer identity at {}: {e}",
                    path.display()
                )
            })?;
            let saved = saved.trim().to_string();
            if Uuid::parse_str(&saved).is_ok() {
                return Ok(saved);
            }
            logln!("printer-uuid was not a uuid; minting a replacement");
        }
    }

    let uuid = Uuid::new_v4().to_string();
    write_config(app, PRINTER_UUID_FILE, &uuid);
    Ok(uuid)
}

/// The name the user set last time, or the hostname-derived default when they
/// have never set one. Empty files count as "never set" rather than as a name,
/// so a half-written file cannot leave the printer called nothing.
fn load_printer_name(app: &tauri::AppHandle) -> Option<String> {
    read_config(app, PRINTER_NAME_FILE).filter(|name| !name.is_empty())
}

/// Whether this machine should be sharing its printer at launch.
/// Defaults to true for print shops unless the user explicitly toggled it off.
fn broadcast_preference(app: &tauri::AppHandle) -> bool {
    read_config(app, BROADCAST_PREF_FILE).as_deref() != Some("off")
}

fn set_broadcast_preference(app: &tauri::AppHandle, on: bool) {
    write_config(app, BROADCAST_PREF_FILE, if on { "on" } else { "off" });
}

// ---------------------------------------------------------------------------
// Local IPP proxy (client role)
// ---------------------------------------------------------------------------

/// The loopback endpoint the installed Windows virtual printer posts to.
fn local_proxy_url() -> String {
    format!("http://{LOOPBACK}:{LOCAL_PROXY_PORT}/ipp/print")
}

/// Binds `addr`, retrying briefly while a listener we just asked to shut down
/// is still closing.
///
/// Stopping a server only *asks* it to exit - axum finishes in-flight requests
/// first - so a stop immediately followed by a start can land in that window
/// and fail with `AddrInUse`, which Windows reports as `os error 10048`. That
/// is a race with ourselves, not a real conflict, and the second of the two
/// listeners (the loopback proxy) is the one that always loses it.
async fn bind_listener_with_retry(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    const ATTEMPTS: u32 = 10;
    let mut attempt = 0;
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt + 1 < ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Brings up the loopback-only IPP proxy the virtual printer points at.
///
/// This is a *second* listener, not a second application: it serves the same
/// route table as the network server, so a document arriving from the Windows
/// spooler over `127.0.0.1` is parsed, relayed and answered exactly like one
/// arriving from a phone. It runs from launch to exit, independently of the
/// broadcast toggle, so `Ctrl+P` keeps working on a PC that never shares its
/// own printer - and so the two servers can never fight over one port.
async fn start_local_proxy(app: &tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    {
        let slot = state
            .local_proxy_shutdown
            .lock()
            .map_err(|_| "local proxy state is locked".to_string())?;
        if slot.is_some() {
            return Ok(local_proxy_url());
        }
    }

    let printer_uuid = {
        let saved = state.printer_uuid.lock().unwrap().trim().to_string();
        if saved.is_empty() {
            // Same identity the network server will read from disk, so both
            // listeners answer with one `printer-uuid` and Windows never
            // decides the device changed.
            load_or_create_printer_uuid(app)?
        } else {
            saved
        }
    };
    let printer_name = state.printer_name.lock().unwrap().clone();

    let ipp_state = IppState {
        app: Some(app.clone()),
        jobs: state.jobs.clone(),
        local_ip: LOOPBACK.to_string(),
        port: LOCAL_PROXY_PORT,
        printer_uuid,
        printer_name,
        relay_host: state.relay_host.clone(),
        selected_printer: state.selected_printer.clone(),
        relay_warning: state.relay_warning.clone(),
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], LOCAL_PROXY_PORT));
    let listener = bind_listener_with_retry(addr).await.map_err(|e| {
        let message = format!("Local proxy port {LOCAL_PROXY_PORT} is busy: {e}");
        if let Ok(mut slot) = state.local_proxy_error.lock() {
            *slot = message.clone();
        }
        message
    })?;

    let (tx, mut rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            ipp_router(ipp_state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = rx.recv().await;
            logln!("Shutting down local IPP proxy.");
        })
        .await;
    });

    {
        let mut slot = state
            .local_proxy_shutdown
            .lock()
            .map_err(|_| "local proxy state is locked".to_string())?;
        *slot = Some(tx);
    }
    if let Ok(mut error) = state.local_proxy_error.lock() {
        error.clear();
    }
    logln!("Local IPP proxy listening on {LOOPBACK}:{LOCAL_PROXY_PORT} (loopback only)");
    Ok(local_proxy_url())
}

/// Stops the loopback proxy. Only ever needed on exit and on a rename, which
/// rebuilds it under the new name.
fn stop_local_proxy(state: &tauri::State<'_, AppState>) {
    if let Ok(mut slot) = state.local_proxy_shutdown.lock() {
        if let Some(tx) = slot.take() {
            let _ = tx.try_send(());
        }
    }
}

/// What the UI shows next to "Install Virtual Printer": whether the queue it
/// is about to create has a server waiting on the other end of it.
#[derive(Serialize)]
struct LocalProxyStatus {
    running: bool,
    port: u16,
    url: String,
    /// Empty while the proxy is running.
    error: String,
}

#[tauri::command]
fn get_local_proxy_status(state: tauri::State<'_, AppState>) -> LocalProxyStatus {
    let running = state
        .local_proxy_shutdown
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(false);
    let error = if running {
        String::new()
    } else {
        state
            .local_proxy_error
            .lock()
            .map(|message| message.clone())
            .unwrap_or_default()
    };
    LocalProxyStatus {
        running,
        port: LOCAL_PROXY_PORT,
        url: local_proxy_url(),
        error,
    }
}

/// Turns whatever the user typed into the host into the URL the proxy posts to.
///
/// Accepts `192.168.1.6`, `192.168.1.6:9000`, `http://192.168.1.6:9000` and
/// anything with a path on the end, and always returns
/// `http://<authority>:<port>/api/relay-print`. Rejecting a malformed address
/// *here* is the point: by the time a job is being forwarded, Windows has
/// already accepted it and a bad address would surface as a spooler error with
/// nothing behind it.
fn relay_url(host: &str) -> Result<String, String> {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return Err("Enter the address of the PC that has the real printer.".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err("Only http:// is supported - Nearby Print does not speak TLS.".into());
    }
    // The scheme is stripped before the trailing slashes are, otherwise
    // `http://` would be trimmed down to `http:` and pass every check below
    // as a host in its own right.
    let rest = lower.strip_prefix("http://").unwrap_or(&lower);
    let rest = rest.trim_end_matches('/');
    let authority = rest.split('/').next().unwrap_or_default();

    if authority.is_empty()
        || !authority
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
    {
        return Err(format!("'{host}' is not a valid host address."));
    }
    // A bare address means the app's own port, so `192.168.1.6` and
    // `192.168.1.6:8631` behave identically instead of one of them timing out.
    let authority = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:{IPP_PORT}")
    };
    Ok(format!("http://{authority}{RELAY_PATH}"))
}

/// One connection pool for every relay attempt. Built lazily because a pure
/// host never forwards anything and should not pay for a client it will not
/// use.
fn relay_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default()
    })
}

/// Records why bringing the printer up failed and hands the same text back to
/// the caller. Without this a broadcast that never starts looks identical to
/// one nobody asked for - the toggle just stays off with no explanation.
fn remember_broadcast_error(state: &tauri::State<'_, AppState>, error: String) -> String {
    if let Ok(mut slot) = state.last_broadcast_error.lock() {
        *slot = error.clone();
    }
    error
}

/// The route table every listener serves.
///
/// The network server and the loopback proxy are one application behind two
/// ports: a document arriving from a phone and one arriving from the Windows
/// spooler over `127.0.0.1` must be parsed, relayed and answered identically,
/// so there is exactly one copy of that logic to get right.
fn ipp_router(ipp_state: IppState) -> Router {
    Router::new()
        .route("/", get(ipp_get).post(ipp_post))
        .route("/portal", get(portal_get))
        .route("/upload", get(portal_get))
        .route("/api/web-print", post(web_print_post))
        .route("/ipp/print", get(ipp_get).post(ipp_post))
        // The relay receiver. Registered before the fallback so an unmatched
        // POST never falls into `ipp_post` and gets parsed as IPP.
        .route(RELAY_PATH, post(relay_print))
        .fallback(get(ipp_get).post(ipp_post))
        // Axum caps request bodies at 2 MB by default; real documents are
        // bigger than that, and a truncated Send-Document body is exactly
        // how a job ends up "received in the terminal but not in the UI".
        // 1 GiB is far beyond any print job while still bounding how much
        // a single request can make us buffer.
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
        .layer(middleware::from_fn(log_request))
        .with_state(Arc::new(ipp_state))
}

/// Bring the printer online: bind the IPP port, register mDNS under the
/// current name, remember that the user wants this on at next launch.
///
/// Split out of `toggle_broadcast` because three paths need exactly this
/// behaviour - the toggle, the startup auto-restore, and a rename, which has
/// to re-register the service under the name it is now called.
async fn start_broadcasting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    if !state.server_url.lock().unwrap().is_empty() {
        return Ok(state.server_url.lock().unwrap().clone());
    }
    *state.last_broadcast_error.lock().unwrap() = String::new();

    // Recorded before the attempt: the user asked for it, so the next
    // launch tries again even if this one hit a busy port. Leaving this
    // unset is what made every fresh launch start with the Windows queue
    // pointed at a port nothing was listening on.
    set_broadcast_preference(&app, true);

    let ip = primary_lan_ip();
    let printer_uuid =
        load_or_create_printer_uuid(&app).map_err(|e| remember_broadcast_error(&state, e))?;
    let printer_name = state.printer_name.lock().unwrap().clone();
    logln!("Starting broadcast on {ip}:{IPP_PORT} (uuid {printer_uuid}, name {printer_name})");

    let ipp_state = IppState {
        app: Some(app.clone()),
        jobs: state.jobs.clone(),
        local_ip: ip.clone(),
        port: IPP_PORT,
        printer_uuid: printer_uuid.clone(),
        printer_name: printer_name.clone(),
        relay_host: state.relay_host.clone(),
        selected_printer: state.selected_printer.clone(),
        relay_warning: state.relay_warning.clone(),
    };

    let (tx, mut rx) = mpsc::channel::<()>(1);
    let addr = SocketAddr::from(([0, 0, 0, 0], IPP_PORT));

    let listener = bind_listener_with_retry(addr).await.map_err(|e| {
        remember_broadcast_error(&state, format!("Port {IPP_PORT} is busy: {e}"))
    })?;

    let router = ipp_router(ipp_state);

    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = rx.recv().await;
            logln!("Shutting down IPP server.");
        })
        .await;
    });

    // Pure-Rust mDNS registration. No `dns-sd` binary and no Apple
    // Bonjour service are involved, so this works on a stock Windows box.
    let mdns = start_mdns_sd(&ip, &printer_name)
        .map_err(|e| remember_broadcast_error(&state, format!("mDNS registration failed: {e}")))?;
    *state.mdns.lock().unwrap() = Some(mdns);

    *state.server_shutdown.lock().unwrap() = Some(tx);
    *state.local_ip.lock().unwrap() = ip.clone();
    *state.printer_uuid.lock().unwrap() = printer_uuid;

    let url = format!("ipp://{ip}:{IPP_PORT}/ipp/print");
    *state.server_url.lock().unwrap() = url.clone();
    Ok(url)
}

#[tauri::command]
async fn toggle_broadcast(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    enable: bool,
) -> Result<String, String> {
    if enable {
        start_broadcasting(app, state).await
    } else {
        logln!("Stopping broadcast...");
        set_broadcast_preference(&app, false);
        stop_broadcast(&state);
        Ok("Broadcasting stopped".to_string())
    }
}

#[tauri::command]
async fn start_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    toggle_broadcast(app, state, true).await
}

#[tauri::command]
async fn stop_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    toggle_broadcast(app, state, false).await
}

fn shutdown_everything(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    stop_broadcast(&state);
    // The loopback proxy and the discovery browse both outlive broadcast
    // cycles, so nothing else would ever stop them.
    stop_local_proxy(&state);
    if let Ok(mut slot) = state.discovery_daemon.lock() {
        if let Some(daemon) = slot.take() {
            let _ = daemon.stop_browse(MDNS_SERVICE_TYPE);
            let _ = daemon.shutdown();
        }
    };
}

// ---------------------------------------------------------------------------
// IPP server
// ---------------------------------------------------------------------------

const PORTAL_HTML: &str = include_str!("portal.html");

async fn portal_get() -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(PORTAL_HTML))
        .unwrap()
}

fn urlencoding_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            if let (Some(h1), Some(h2)) = (chars.next(), chars.next()) {
                let hex_str = [h1, h2];
                if let Ok(s) = std::str::from_utf8(&hex_str) {
                    if let Ok(val) = u8::from_str_radix(s, 16) {
                        bytes.push(val);
                        continue;
                    }
                }
            }
        } else if b == b'+' {
            bytes.push(b' ');
            continue;
        }
        bytes.push(b);
    }
    String::from_utf8(bytes).unwrap_or_else(|_| input.to_string())
}

async fn web_print_post(
    State(state): State<Arc<IppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let reply = |status: u16, payload: serde_json::Value| {
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };

    if body.is_empty() {
        return reply(400, serde_json::json!({ "ok": false, "error": "Empty file received" }));
    }

    let header_str = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };

    let raw_customer = header_str("x-nearby-customer").unwrap_or_else(|| "Customer".into());
    let customer = urlencoding_decode(&raw_customer);

    let raw_job_name = header_str("x-nearby-job-name").unwrap_or_else(|| "document.pdf".into());
    let job_name = urlencoding_decode(&raw_job_name);

    let copies = header_str("x-nearby-copies")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .clamp(1, 99);

    let color_mode = header_str("x-nearby-color").unwrap_or_else(|| "bw".into());
    let is_card = header_str("x-nearby-is-card").map(|v| v == "true").unwrap_or(false);

    let content_type = header_str("content-type").unwrap_or_else(|| "application/pdf".into());
    let (path, filename, ext, size) = save_document_to_temp(&body, &content_type);
    let path_str = path.to_string_lossy().to_string();

    let job_id = next_job_id();
    let display_name = if job_name.trim().is_empty() {
        filename
    } else {
        job_name
    };

    let options = PrintOptions {
        copies,
        orientation: ORIENTATION_PORTRAIT,
    };

    let card_tag = if is_card { " [ID Card CR80]" } else { "" };
    let info = JobInfo {
        id: job_id,
        name: display_name,
        device: format!("Web: {}", customer),
        user: customer.clone(),
        pages: 1,
        bytes: size,
        format: ext,
        status: "pending".to_string(),
        message: format!(
            "Web Upload from {}{} ({} cop{}, {})",
            customer,
            card_tag,
            copies,
            if copies > 1 { "ies" } else { "y" },
            if color_mode == "color" { "Color" } else { "B&W" }
        ),
        printer: String::new(),
        file_path: Some(path_str),
        channel: Some("web".into()),
        customer: Some(customer.clone()),
        color_mode: Some(color_mode),
        is_card: Some(is_card),
    };

    {
        let mut map = state.jobs.map.lock().unwrap();
        map.insert(
            job_id,
            Job {
                info: info.clone(),
                path: Some(path),
                ipp_state: JOB_PENDING,
                options,
            },
        );
    }

    emit_job(&state, &info, "new-print-job");
    logln!("WEB-PORTAL: received {size}B for customer '{customer}' -> Job {job_id}");

    reply(200, serde_json::json!({
        "ok": true,
        "job_id": job_id,
        "customer": customer,
        "message": "Print queued at counter"
    }))
}

/// A GET is never a print job - it is a reachability probe from a client (or a
/// browser) checking the URL before it commits to a POST. It used to be
/// answered silently, which made "did *anything* at all reach us?" impossible
/// to answer while chasing a client that insisted it was printing to us.
async fn ipp_get(
    connect: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    logln!("HTTP GET {} from {}", uri.path(), connect.0.ip());
    let accept = headers.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("");
    if uri.path() == "/portal" || uri.path() == "/upload" || accept.contains("text/html") {
        return portal_get().await;
    }
    Response::builder()
        .status(200)
        .header("Content-Type", "text/plain")
        .body(Body::from("Nearby Print IPP Server is Online"))
        .unwrap()
}

/// Records a request the instant it lands - *before* axum has read its body.
///
/// `ipp_post` only runs once the entire body has arrived, and a client that
/// connects and then stalls, is cut off mid-upload, or is answered with a
/// connection-level error leaves no trace in either `ipp_post` or
/// `build_response`. That gap is exactly what makes "the spooler never sent the
/// job" and "the job was sent and we dropped it" indistinguishable while
/// diagnosing Windows, so every non-GET is logged at arrival and the two cases
/// can finally be told apart by one line.
async fn log_request(req: Request, next: Next) -> Response {
    if req.method() == Method::GET {
        return next.run(req).await;
    }

    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let method = req.method().clone();
    let uri = req.uri().clone();

    // The framing headers are the whole point here: a request that declares a
    // body and then never delivers it leaves `ipp_post` uninvoked, so without
    // these the arrival and the silence that follows say nothing about which
    // side gave up.
    let framing = {
        let h = req.headers();
        let get = |k: &str| {
            h.get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-")
                .to_string()
        };
        format!(
            "len={} type={} expect={} transfer={}",
            get("content-length"),
            get("content-type"),
            get("expect"),
            get("transfer-encoding"),
        )
    };
    logln!("REQ {method} {uri} from {peer} ({framing})");

    let resp = next.run(req).await;
    logln!("REQ done {method} {uri} -> {}", resp.status());
    resp
}

async fn ipp_post(
    connect: ConnectInfo<SocketAddr>,
    State(state): State<Arc<IppState>>,
    body: Bytes,
) -> Response {
    // When this process is configured as a relay client, a document-bearing
    // Print-Job / Send-Document is posted to the host *before* the reply is
    // built - the spooler's own result has to say whether the far side took
    // the job, and answering "printed" for a job that never left this machine
    // is how a print silently disappears.
    let built = match relay_document(&state, &body, connect.0).await {
        Some(result) => result,
        None => build_response(&state, &body, connect.0),
    };

    match built {
        Ok(bytes) => Response::builder()
            .status(200)
            .header("Content-Type", "application/ipp")
            .body(Body::from(bytes))
            .unwrap(),
        Err(status) => Response::builder()
            .status(status)
            .body(Body::from("Bad Request"))
            .unwrap(),
    }
}

/// `server-error-service-unavailable` (RFC 8010 6.2.8). What a spooler gets
/// when the host could not be reached, so the queue shows a failed job with a
/// real reason instead of a completed one with nothing behind it.
const STATUS_SERVICE_UNAVAILABLE: u16 = 0x0508;

/// Keeps a failure reason short and 7-bit so it can go into the IPP
/// `status-message` attribute without the conversion panicking on whatever
/// text a transport error happened to produce.
fn status_text(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' })
        .take(200)
        .collect();
    out.shrink_to_fit();
    out
}

/// Posts one document to the host. Pure HTTP on the LAN, so the reply is a
/// plain status code - anything else is reported back to the spooler verbatim.
async fn send_document_to_host(
    url: &str,
    document: &[u8],
    format: &str,
    filename: &str,
    job_id: u32,
    peer: SocketAddr,
    options: PrintOptions,
) -> Result<(), RelayFailure> {
    let content_type = if format.trim().is_empty() {
        "application/pdf"
    } else {
        format.trim()
    };

    let response = relay_client()
        .post(url)
        .header("Content-Type", content_type)
        .header("X-Nearby-Job-Name", filename)
        .header("X-Nearby-Job-Id", job_id.to_string())
        .header("X-Nearby-Source", peer.ip().to_string())
        // The relay POST is plain document bytes, so the job-template values
        // the IPP client asked for travel as headers - without them the host
        // printed one portrait copy of every relayed job.
        .header("X-Nearby-Copies", options.copies.to_string())
        .header("X-Nearby-Orientation", options.orientation.to_string())
        .body(document.to_vec())
        .send()
        .await
        .map_err(|e| RelayFailure::Unreachable(format!("could not reach the printer PC: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        // The receiver puts the reason in `error`; anything else (a proxy 404,
        // an HTML error page) is not worth forwarding to the spooler.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(str::to_string))
            .unwrap_or_default();
        return Err(RelayFailure::Refused(if detail.is_empty() {
            format!("the printer PC refused the job ({status})")
        } else {
            format!("the printer PC refused the job: {}", status_text(&detail))
        }));
    }
    logln!("RELAY: job {job_id} accepted by the printer PC ({status})");
    Ok(())
}

/// Why a relayed document did not reach the printer PC.
///
/// The split matters: only an *unreachable* host means the document reached
/// nobody, and only then can printing it on this machine instead be guaranteed
/// not to produce a second copy.
enum RelayFailure {
    /// The host could not be contacted at all - powered off, different
    /// network, an IP it no longer has.
    Unreachable(String),
    /// The host answered and turned the job down. It knows about the
    /// document, so printing here as well could duplicate it.
    Refused(String),
}

impl RelayFailure {
    fn reason(&self) -> &str {
        match self {
            RelayFailure::Unreachable(r) | RelayFailure::Refused(r) => r,
        }
    }
}

/// Records that the configured printer PC could not be reached, and tells the
/// UI about it.
///
/// Without this the failure is invisible: the job lands on this machine's
/// printer instead of the one the user chose, and nothing on screen ever says
/// so. The warning is cleared again by a relay that succeeds, so it names a
/// host that is *currently* not answering rather than one that once failed.
fn notify_relay_fallback(state: &Arc<IppState>, host: &str, reason: &str) {
    let message = format!("Relay host {host} is unreachable: {reason}");
    *state.relay_warning.lock().unwrap() = message.clone();
    if let Some(app) = &state.app {
        let _ = app.emit("relay-warning", message);
    }
}

/// Clears the unreachable-host warning: the configured host took a job.
fn clear_relay_warning(state: &Arc<IppState>) {
    let mut warning = state.relay_warning.lock().unwrap();
    if !warning.is_empty() {
        *warning = String::new();
        if let Some(app) = &state.app {
            let _ = app.emit("relay-warning", String::new());
        }
    }
}

/// What came of printing a relay job on this machine after the printer PC
/// could not be reached.
enum LocalFallback {
    /// Handed to this machine's queue.
    Printed(String),
    /// No queue here is safe to print on (none chosen, or the choice is this
    /// app's own virtual printer, which would send the job back out again).
    NoPrinter,
    /// A queue was chosen but Windows refused it.
    Failed(String),
}

/// Last resort for a job whose printer PC never answered: print it here.
///
/// The document reached *nobody* on the wire, so one copy on this machine's
/// queue is still exactly one copy. Anything else - the job silently
/// vanishing, or the spooler claiming success for a document that is sitting
/// in a temp directory - is worse than printing on the "wrong" printer.
async fn local_fallback(
    state: &Arc<IppState>,
    path: &Path,
    title: &str,
    options: PrintOptions,
) -> LocalFallback {
    let chosen = state.selected_printer.lock().unwrap().trim().to_string();
    // The app's own virtual printer must never be used: it posts back into
    // this very proxy, and a job routed there would loop until the spooler
    // gives up. The relay *receiver* checks this too - see `relay_print` -
    // but that check only runs on the far side, and this one has to hold even
    // when there is no far side.
    if chosen.is_empty() || is_our_own_queue(&chosen, &state.printer_name) {
        return LocalFallback::NoPrinter;
    }
    let Some(app) = state.app.clone() else {
        return LocalFallback::NoPrinter;
    };
    let file = path.to_path_buf();
    let printer = chosen;
    let job_title = title.to_string();
    let printed_on = printer.clone();
    match tokio::task::spawn_blocking(move || {
        send_to_printer(&app, &printer, &file, &job_title, options)
    })
    .await
    {
        Ok(Ok(_)) => LocalFallback::Printed(printed_on),
        Ok(Err(e)) => LocalFallback::Failed(e),
        Err(e) => LocalFallback::Failed(format!("local print task failed: {e}")),
    }
}

/// The client half of the relay: forwards a received document and builds the
/// IPP reply for it.
///
/// Returns `None` for every request that must not be forwarded - no host
/// configured (this PC is a host), discovery and job-management operations, or
/// a payload that never arrived - and `Some` for everything else, so exactly
/// one of the two paths ever answers a request.
async fn relay_document(
    state: &Arc<IppState>,
    body: &[u8],
    peer: SocketAddr,
) -> Option<Result<Vec<u8>, u16>> {
    let host = state.relay_host.lock().unwrap().trim().to_string();
    if host.is_empty() || body.len() < 8 {
        return None;
    }

    let operation_id = u16::from_be_bytes([body[2], body[3]]);
    if !matches!(operation_id, OP_PRINT_JOB | OP_SEND_DOCUMENT) {
        return None;
    }

    let parser: IppParser<std::io::Cursor<Vec<u8>>> =
        IppParser::new(std::io::Cursor::new(body.to_vec()));
    let (header, attrs, reader) = parser.parse_parts().ok()?;
    let mut document = Vec::new();
    if reader.into_payload().read_to_end(&mut document).is_err() || document.is_empty() {
        // No bytes: `build_response` already knows how to report that for a
        // job it holds, and inventing a second answer would contradict it.
        return None;
    }

    // macOS queues send PostScript, which the host cannot print either, so the
    // conversion happens before the hand-off rather than being passed through.
    let document_format = req_str(&attrs, "document-format").unwrap_or_default();
    match prepare_document(&document_format, &document) {
        Ok(Some(pdf)) => {
            logln!(
                "RELAY: converted PostScript -> PDF ({} -> {} bytes)",
                document.len(),
                pdf.len()
            );
            document = pdf;
        }
        Ok(None) => {}
        Err(reason) => {
            // Not relayable and not printable here either. Falling through
            // gives the spooler the same `document-format-not-supported` it
            // has always had, from the code that already writes that reply.
            logln!("RELAY: not forwarding: {reason}");
            return None;
        }
    }

    // Keep the id the client is already tracking when we know it (a two-step
    // Create-Job + Send-Document), and mint one otherwise.
    let requested = req_job_id(&attrs).filter(|&id| id > 0);
    let known_job = requested.filter(|id| state.jobs.map.lock().unwrap().contains_key(id));
    let job_id = known_job.or(requested).unwrap_or_else(next_job_id);

    // Copies / orientation the IPP client asked for. A Create-Job that opened
    // this job may have carried them, so the existing job's options are the
    // base this request merges into.
    let base_options = known_job
        .and_then(|id| {
            state
                .jobs
                .map
                .lock()
                .unwrap()
                .get(&id)
                .map(|job| job.options)
        })
        .unwrap_or_default();
    let options = PrintOptions::from_request(&attrs, base_options);

    let (path, filename, ext, size) = save_document_to_temp(&document, &document_format);
    let job_name = req_str(&attrs, "job-name")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| filename.clone());

    let outcome = match relay_url(&host) {
        Ok(url) => {
            logln!(
                "RELAY: job {job_id} - {size}B '{job_name}' ({}) -> {url}",
                options.describe()
            );
            send_document_to_host(
                &url,
                &document,
                &document_format,
                &filename,
                job_id,
                peer,
                options,
            )
            .await
        }
        // A host stored in a shape `relay_url` cannot build was never contacted
        // either, so - like an unreachable host - it leaves the document with
        // nobody, and printing here is still a single copy.
        Err(e) => Err(RelayFailure::Unreachable(e)),
    };

    // The printer PC never answered: the document reached nobody, so this
    // machine prints it instead. A host that *refused* is deliberately not
    // retried here - it has the document, and a second copy is worse than a
    // reported failure.
    let mut delivered_message: Option<String> = None;
    let outcome = match outcome {
        Err(RelayFailure::Unreachable(reason)) => {
            logln!("RELAY: job {job_id} unreachable ({reason}) - trying this PC instead");
            notify_relay_fallback(state, &host, &reason);
            match local_fallback(state, &path, &job_name, options).await {
                LocalFallback::Printed(printer) => {
                    let message =
                        format!("Printer PC unreachable - printed on '{printer}' instead");
                    logln!("RELAY: job {job_id} {message}");
                    delivered_message = Some(message);
                    Ok(())
                }
                LocalFallback::NoPrinter => {
                    let message = format!(
                        "{reason} (no printer selected on this PC to fall back to)"
                    );
                    Err(RelayFailure::Unreachable(message))
                }
                LocalFallback::Failed(e) => {
                    let message = format!("{reason} (local fallback failed: {e})");
                    Err(RelayFailure::Unreachable(message))
                }
            }
        }
        other => other,
    };

    let ok = outcome.is_ok();
    // Only a job the configured host actually took proves the host is back;
    // one printed here by the fallback proves the opposite.
    let host_took_it = ok && delivered_message.is_none();
    let message = match &outcome {
        Ok(()) => delivered_message.unwrap_or_else(|| format!("Forwarded to {host}")),
        Err(e) => e.reason().to_string(),
    };
    if host_took_it {
        clear_relay_warning(state);
    }
    logln!(
        "RELAY: job {job_id} {} - {message}",
        if ok { "handed over" } else { "failed" }
    );

    let mut resp = match IppRequestResponse::new_response(
        header.version,
        StatusCode::SuccessfulOk,
        header.request_id,
    ) {
        Ok(resp) => resp,
        Err(_) => return Some(Err(500u16)),
    };
    resp.header_mut().operation_or_status = if ok {
        StatusCode::SuccessfulOk as u16
    } else {
        STATUS_SERVICE_UNAVAILABLE
    };
    resp.attributes_mut().add(
        DelimiterTag::OperationAttributes,
        IppAttribute::new(
            "status-message".try_into().unwrap(),
            IppValue::TextWithoutLanguage(status_text(&message).as_str().try_into().unwrap()),
        ),
    );
    add_job_group(
        &mut resp,
        state,
        job_id,
        if ok {
            RFC_JOB_STATE_COMPLETED
        } else {
            RFC_JOB_STATE_ABORTED
        },
    );

    // A job that failed stays on disk so it can be printed from the app's own
    // list instead of having to be sent again; a job that went out is not
    // duplicated locally.
    let keep_path = if ok {
        let _ = std::fs::remove_file(&path);
        None
    } else {
        Some(path)
    };

    let had_job = known_job.is_some();
    let info = JobInfo {
        id: job_id,
        name: job_name,
        device: host.clone(),
        user: "unknown".to_string(),
        pages: 1,
        bytes: size,
        format: ext,
        status: if ok { "completed" } else { "error" }.to_string(),
        message: message.clone(),
        printer: host,
        file_path: keep_path.as_ref().map(|p| p.to_string_lossy().to_string()),
        channel: Some("relay".into()),
        customer: None,
        color_mode: None,
        is_card: Some(false),
    };
    {
        let mut map = state.jobs.map.lock().unwrap();
        map.insert(
            job_id,
            Job {
                info: info.clone(),
                path: keep_path,
                ipp_state: if ok { JOB_COMPLETED } else { JOB_ABORTED },
                options,
            },
        );
    }
    // `job_updated` for a job the client opened with Create-Job, `new-print-job`
    // for one that arrived with its payload attached - the two events the front
    // end already funnels into the same list.
    emit_job(state, &info, if had_job { "job_updated" } else { "new-print-job" });

    Some(Ok(resp.to_bytes().to_vec()))
}

/// The host half of the relay: spools a document a client PC just posted and
/// starts printing it, then answers straight away so the client's queue does
/// not sit blocked behind this machine's paper speed.
async fn relay_print(
    State(state): State<Arc<IppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let reply = |status: u16, payload: serde_json::Value| {
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };

    let source = header("x-nearby-source").unwrap_or_else(|| "relay".into());
    let sent_name = header("x-nearby-job-name").unwrap_or_default();
    let format = header("content-type")
        .unwrap_or_else(|| "application/pdf".into())
        .split(';')
        .next()
        .unwrap_or("application/pdf")
        .trim()
        .to_string();

    // Copies / orientation the client asked for, travelled as headers (see
    // `send_document_to_host`). A sender that does not send them - an older
    // NearbyPrint build, or a hand-made POST - still gets one portrait copy.
    let options = PrintOptions {
        copies: header("x-nearby-copies")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1)
            .clamp(1, 999),
        orientation: header("x-nearby-orientation")
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|o| (ORIENTATION_PORTRAIT..=ORIENTATION_REVERSE_PORTRAIT).contains(o))
            .unwrap_or(ORIENTATION_PORTRAIT),
    };

    if body.is_empty() {
        logln!("RELAY: rejected an empty document from {source}");
        return reply(400, serde_json::json!({ "ok": false, "error": "empty document" }));
    }

    let (path, filename, ext, size) = save_document_to_temp(&body, &format);
    let path_str = path.to_string_lossy().to_string();

    // Only a real, reachable queue is printed from here. A queue that belongs
    // to this process - or none at all - falls back to the ordinary "pending
    // job" route instead, because handing the document to our own queue would
    // send it straight back into the relay it just came from, forever. On
    // Windows the device-URI check below has never worked (it is `lpstat`), so
    // the name check is what actually holds that door shut.
    let chosen = state.selected_printer.lock().unwrap().trim().to_string();
    let device_name = state.printer_name.clone();
    let ours = !chosen.is_empty()
        && (is_our_own_queue(&chosen, &device_name)
            || device_uri_of(&chosen).map(|u| is_self_uri(&u)).unwrap_or(false));
    let printer = if ours { String::new() } else { chosen.clone() };

    let job_id = next_job_id();

    // Printing happens here rather than being left to the front end, so the
    // job is announced as `printing` and the auto-print - which only ever
    // picks up `pending` jobs - cannot put the same document on the paper
    // twice. When there is no safe queue to print on, it is announced as
    // `pending` instead and the ordinary path takes over.
    let will_print = !printer.is_empty() && state.app.is_some();
    let info = JobInfo {
        id: job_id,
        name: if sent_name.trim().is_empty() {
            filename.clone()
        } else {
            sent_name
        },
        device: source.clone(),
        user: "relay".to_string(),
        pages: 1,
        bytes: size,
        format: ext,
        status: if will_print { "printing" } else { "pending" }.to_string(),
        message: if ours {
            format!(
                "Relayed from {source} ({size} bytes) - held: '{chosen}' is Nearby Print's own queue"
            )
        } else {
            format!("Relayed from {source} ({size} bytes)")
        },
        printer: printer.clone(),
        file_path: Some(path_str.clone()),
        channel: Some("relay".into()),
        customer: None,
        color_mode: None,
        is_card: Some(false),
    };
    {
        let mut map = state.jobs.map.lock().unwrap();
        map.insert(
            job_id,
            Job {
                info: info.clone(),
                path: Some(path),
                ipp_state: if will_print {
                    JOB_PROCESSING
                } else {
                    JOB_PENDING
                },
                options,
            },
        );
    }
    emit_job(&state, &info, "new-print-job");

    logln!(
        "RELAY: received {size}B from {source} -> '{}' ({})",
        info.name,
        if printer.is_empty() {
            "queued for the UI".to_string()
        } else {
            format!("printing on '{printer}'")
        }
    );

    if !will_print {
        // Nothing safe to print on: leave it queued, exactly like a job that
        // arrived over the network.
        return reply(
            200,
            serde_json::json!({ "ok": true, "file": filename, "bytes": size, "job_id": job_id }),
        );
    }

    if let Some(app) = state.app.clone() {
        let state_c = state.clone();
        let printer_c = printer;
        let title = info.name.clone();
        let file = PathBuf::from(&path_str);
        tokio::task::spawn_blocking(move || {
            let (status, message) = match send_to_printer(&app, &printer_c, &file, &title, options)
            {
                Ok(_) => (
                    "completed",
                    format!("Printed ({}) on {printer_c}", options.describe()),
                ),
                Err(e) => ("error", e),
            };
            let info = state_c.jobs.update(job_id, |j| {
                j.info.status = status.into();
                j.info.message = message.clone();
                j.ipp_state = if status == "completed" {
                    JOB_COMPLETED
                } else {
                    JOB_ABORTED
                };
                if status == "completed" {
                    if let Some(p) = j.path.take() {
                        let _ = std::fs::remove_file(p);
                    }
                    j.info.file_path = None;
                }
            });
            if let Some(info) = info {
                emit_job(&state_c, &info, "job_updated");
            }
        });
    }

    reply(
        200,
        serde_json::json!({ "ok": true, "file": filename, "bytes": size, "job_id": job_id }),
    )
}

fn build_response(state: &IppState, body: &[u8], peer: SocketAddr) -> Result<Vec<u8>, u16> {
    // IPP requests are 8-byte headers plus attribute groups; anything shorter
    // is malformed and must not reach the indexing below.
    if body.len() < 8 {
        logln!("IPP: rejecting {}B request from {} (too short)", body.len(), peer.ip());
        return Err(400u16);
    }

    let parser: IppParser<std::io::Cursor<Vec<u8>>> =
        IppParser::new(std::io::Cursor::new(body.to_vec()));
    // A request the crate cannot parse used to vanish without a trace, because
    // the one log line that identifies the operation sits *after* this point.
    // That turns "the client never sent the job" into "the client sent
    // something we silently threw away" - indistinguishable from a distance.
    let (header, attrs, reader) = match parser.parse_parts() {
        Ok(parts) => parts,
        Err(e) => {
            logln!(
                "IPP: unparsable {}B request (operation 0x{:04X}) from {}: {e}",
                body.len(),
                u16::from_be_bytes([body[2], body[3]]),
                peer.ip()
            );
            return Err(400u16);
        }
    };

    // Everything after the end-of-attributes delimiter is the document - this
    // is what Send-Document (0x0006) and Print-Job (0x0002) both carry.
    let mut payload = reader.into_payload();
    let mut document = Vec::new();
    if let Err(e) = payload.read_to_end(&mut document) {
        logln!("IPP: failed reading document payload from {}: {e}", peer.ip());
        return Err(400u16);
    }

    let operation_id = u16::from_be_bytes([body[2], body[3]]);

    let op_name = match operation_id {
        OP_GET_PRINTER_ATTRS => "Get-Printer-Attributes",
        OP_GET_JOBS => "Get-Jobs",
        OP_PRINT_JOB => "Print-Job",
        OP_SEND_DOCUMENT => "Send-Document",
        OP_CANCEL_JOB => "Cancel-Job",
        OP_GET_JOB_ATTRS => "Get-Job-Attributes",
        OP_CREATE_JOB => "Create-Job",
        OP_VALIDATE_JOB => "Validate-Job",
        _ => "Unknown",
    };
    logln!(
        "IPP {} (0x{:04X}) req_id={} doc={}B from {}",
        op_name,
        operation_id,
        header.request_id,
        document.len(),
        peer.ip()
    );

    // Windows spoolers send Validate-Job (0x0004) while probing and adding the
    // printer. Per RFC 8011 4.2.5 the response carries no printer or job
    // attributes at all - only the status. Anything other than SuccessfulOk
    // here makes the spooler abort setup and hide the printer, so answer it
    // immediately and explicitly before any other routing.
    if operation_id == OP_VALIDATE_JOB {
        let resp = IppRequestResponse::new_response(
            header.version,
            StatusCode::SuccessfulOk,
            header.request_id,
        )
        .map_err(|_| 500u16)?;
        logln!("IPP Validate-Job -> SuccessfulOk (0x0000) from {}", peer.ip());
        return Ok(resp.to_bytes().to_vec());
    }

    let printer_uri = req_str(&attrs, "printer-uri").unwrap_or_else(|| default_printer_uri(state));

    // macOS queues installed with a PostScript PPD send `%!PS` payloads, and
    // neither the silent sidecar nor `Start-Process` can render PostScript -
    // that is what made Mac jobs die with an invalid-argument error. Convert
    // to a real PDF whenever Ghostscript is available; otherwise remember why
    // the document can never go out, so the job is reported honestly below.
    let document_format = req_str(&attrs, "document-format").unwrap_or_default();
    let mut unprintable: Option<String> = None;
    if matches!(operation_id, OP_PRINT_JOB | OP_SEND_DOCUMENT | OP_CREATE_JOB)
        && !document.is_empty()
    {
        match prepare_document(&document_format, &document) {
            Ok(Some(pdf)) => {
                logln!(
                    "IPP: converted PostScript -> PDF ({} -> {} bytes)",
                    document.len(),
                    pdf.len()
                );
                document = pdf;
            }
            Ok(None) => {}
            Err(reason) => {
                logln!("IPP: document cannot be printed: {reason}");
                unprintable = Some(reason);
            }
        }
    }

    let mut resp = IppRequestResponse::new_response(
        header.version,
        StatusCode::SuccessfulOk,
        header.request_id,
    )
    .map_err(|_| 500u16)?;

    resp.attributes_mut().add(
        DelimiterTag::OperationAttributes,
        IppAttribute::new(
            "status-message".try_into().unwrap(),
            IppValue::TextWithoutLanguage("successful-ok".try_into().unwrap()),
        ),
    );

    match operation_id {
        OP_GET_PRINTER_ATTRS => {
            add_printer_attrs(&mut resp, state, &printer_uri);
        }

        OP_GET_JOBS => {
            // RFC 8011 4.3.5: the reply repeats `number-of-jobs` and carries one
            // Job Attributes group per job. Answering with nothing at all is not
            // read as "the printer is idle" by a client that has just handed us a
            // document - it cannot find the job it is waiting on, asks again, and
            // once it has asked enough times submits the whole file a second time.
            // That is how a single print showed up in this server's log as five.
            let which = req_str(&attrs, "which-jobs").unwrap_or_default().to_lowercase();
            let limit = usize::try_from(req_int(&attrs, "limit").unwrap_or(i32::MAX))
                .unwrap_or(usize::MAX);

            let mut jobs: Vec<(u32, i32, String, String)> = {
                let map = state.jobs.map.lock().unwrap();
                let mut all: Vec<(u32, i32, String, String)> = map
                    .iter()
                    .map(|(&id, j)| {
                        (
                            id,
                            rfc_job_state(j.ipp_state),
                            j.info.name.clone(),
                            j.info.user.clone(),
                        )
                    })
                    .collect();
                all.sort_by_key(|&(id, ..)| id);
                all
            }
            .into_iter()
            .filter(|&(_, s, _, _)| match which.as_str() {
                "completed" => is_terminal_job_state(s),
                "not-completed" => !is_terminal_job_state(s),
                _ => true,
            })
            .collect();
            jobs.truncate(limit);

            resp.attributes_mut().add(
                DelimiterTag::OperationAttributes,
                IppAttribute::new(
                    "number-of-jobs".try_into().unwrap(),
                    IppValue::Integer(jobs.len() as i32),
                ),
            );

            for (id, job_state, name, user) in jobs {
                add_job_group(&mut resp, state, id, job_state);
                add_job_name(&mut resp, &name, &user);
            }
        }

        // Two-step spooling: macOS/iOS send Create-Job (0x0005) to open a job
        // and then push the bytes with Send-Document (0x0006), while the
        // Windows spooler and most Android clients send Print-Job (0x0002)
        // with the payload already attached. All three share this path so the
        // mandatory job attributes (job-id / job-uri / job-state) and the
        // `new-print-job` event are produced identically however the client
        // chose to speak.
        OP_CREATE_JOB | OP_PRINT_JOB | OP_SEND_DOCUMENT => {
            let carried_document = !document.is_empty();
            let is_create_job = operation_id == OP_CREATE_JOB;

            // Attach to the job this client opened with Create-Job when we
            // actually hold it - never to an id we did not hand out ourselves.
            // When the client names an id we do not know (the app restarted
            // mid-job, or a queue was re-added), the response still has to
            // echo *its* job-id: answering with a freshly minted one makes the
            // spooler track a job that does not exist on either side.
            let requested = req_job_id(&attrs).filter(|&id| id > 0);
            let known_job = requested
                .filter(|id| state.jobs.map.lock().unwrap().contains_key(id));
            let had_job = known_job.is_some();
            let job_id = known_job.or(requested).unwrap_or_else(next_job_id);

            // Job-template values the client chose in its print dialog.
            // `copies` and `orientation-requested` live in the request's job
            // group; they used to be read by nobody, so every job printed one
            // portrait copy. A Create-Job that opened this job may have
            // carried them while the Send-Document that delivers the bytes
            // does not repeat them, so the stored job's options are the base
            // this request merges into.
            let base_options = known_job
                .and_then(|id| {
                    state
                        .jobs
                        .map
                        .lock()
                        .unwrap()
                        .get(&id)
                        .map(|job| job.options)
                })
                .unwrap_or_default();
            let options = PrintOptions::from_request(&attrs, base_options);
            if options.copies > 1 || options.orientation != ORIENTATION_PORTRAIT {
                logln!(
                    "IPP: job {job_id} template asks for {} ({})",
                    options.copies,
                    options.describe()
                );
            }

            let mut emit: Option<(&str, JobInfo)> = None;

            if carried_document {
                // One naming path for Print-Job (0x0002) and Send-Document
                // (0x0006) alike: the sidecar only accepts `.pdf`, so a
                // spooled file is never allowed to end in `.bin` again.
                let (path, filename, ext, file_size) =
                    save_document_to_temp(&document, &document_format);
                let file_path = path.to_string_lossy().to_string();

                // A document we could not make printable must not reach the
                // auto-printer: park it as an error carrying the reason.
                let rejected = unprintable.clone();
                let message = match &rejected {
                    Some(reason) => reason.clone(),
                    None => format!("Received ({file_size} bytes)"),
                };

                let info = JobInfo {
                    id: job_id,
                    name: filename.clone(),
                    device: peer.ip().to_string(),
                    user: "unknown".to_string(),
                    pages: 1,
                    bytes: file_size,
                    format: ext,
                    status: if rejected.is_some() {
                        "error".into()
                    } else {
                        "pending".into()
                    },
                    message,
                    printer: String::new(),
                    file_path: Some(file_path.clone()),
                    channel: Some("wifi".into()),
                    customer: None,
                    color_mode: None,
                    is_card: Some(false),
                };

                // Add job to map so print_file can find it. The job is owned
                // by the print pipeline from here on, so it enters as
                // `processing` - the same state the response below reports,
                // and the state Get-Job-Attributes keeps reporting until the
                // UI finishes printing and it flips to `completed`.
                {
                    let mut map = state.jobs.map.lock().unwrap();
                    map.insert(
                        job_id,
                        Job {
                            info: info.clone(),
                            path: Some(path),
                            ipp_state: if rejected.is_some() {
                                JOB_ABORTED
                            } else {
                                JOB_PROCESSING
                            },
                            options,
                        },
                    );
                }

                match &rejected {
                    Some(reason) => {
                        logln!("IPP: job {job_id} stored but not printable: {reason}")
                    }
                    None => logln!(
                        "PRINT JOB RECEIVED SUCCESSFULLY! file={file_path} size={file_size}"
                    ),
                }
                emit = Some(("new-print-job", info));
            } else if is_create_job {
                // Create-Job carries no payload: park an empty placeholder so
                // the follow-up Send-Document can attach to it (and so the job
                // shows up in the UI while it waits for its document).
                let info = JobInfo {
                    id: job_id,
                    name: format!("Job {job_id}"),
                    device: peer.ip().to_string(),
                    user: "unknown".to_string(),
                    pages: 1,
                    bytes: 0,
                    format: String::new(),
                    status: "pending".into(),
                    message: "Waiting for document…".into(),
                    printer: String::new(),
                    file_path: None,
                    channel: Some("wifi".into()),
                    customer: None,
                    color_mode: None,
                    is_card: Some(false),
                };
                if !had_job {
                    let mut map = state.jobs.map.lock().unwrap();
                    map.entry(job_id).or_insert_with(|| Job {
                        info: info.clone(),
                        path: None,
                        ipp_state: JOB_PENDING,
                        // Kept from the Create-Job so the Send-Document that
                        // follows can merge its own template into them - see
                        // `PrintOptions::from_request`.
                        options,
                    });
                    logln!("IPP Create-Job {job_id} opened from {}", peer.ip());
                    emit = Some(("new-print-job", info));
                }
            } else if had_job {
                // Send-Document / Print-Job without a payload: the job exists
                // but no bytes came through, so surface that instead of
                // dropping the job silently.
                if let Some(info) = state.jobs.update(job_id, |j| {
                    // The template may have arrived (or changed) on this
                    // request even though the payload did not.
                    j.options = options;
                    j.info.message =
                        "Document transfer ended without a payload".to_string();
                }) {
                    logln!("IPP: job {job_id} received no document payload");
                    emit = Some(("job_updated", info));
                }
            }

            if let Some((event, info)) = emit {
                emit_job(state, &info, event);
            }

            // Create-Job is answered while the job is still being fed
            // (pending: open, waiting for its document). Print-Job and
            // Send-Document are answered once the bytes are in, as
            // `processing` (5) - the job is now ours and the print pipeline
            // owns it; it becomes `completed` (9) when the client polls
            // Get-Job-Attributes after the UI has actually printed. Answering
            // `completed` on submission, as this used to, made the Mac mark
            // the job done before a single page came out, so a failed print
            // still showed as a success in the Mac's own job history.
            //
            // All three are success responses, and all three must carry
            // job-id / job-uri / job-state (RFC 8011 4.3.3 and 4.3.4) or the
            // Windows spooler records the submission as refused and flips the
            // queue to "Printing, Error" on the spot.
            let job_state = match state.jobs.map.lock().unwrap().get(&job_id) {
                Some(job) => rfc_job_state(job.ipp_state),
                // The id was echoed back for a job we no longer hold: report
                // processing rather than a completed job nobody printed.
                None if is_create_job => RFC_JOB_STATE_PENDING,
                None => RFC_JOB_STATE_PROCESSING,
            };
            add_job_group(&mut resp, state, job_id, job_state);
            add_job_name(
                &mut resp,
                &req_str(&attrs, "job-name").unwrap_or_else(|| format!("Job {job_id}")),
                &req_str(&attrs, "job-originating-user-name").unwrap_or_else(|| "unknown".into()),
            );
        }

        // 0x0004 (Validate-Job) is handled by the explicit early return above.
        OP_CANCEL_JOB | OP_PRINT_URI
        | OP_PAUSE_PRINTER | OP_RESUME_PRINTER | OP_PURGE_JOBS => {}

        OP_GET_JOB_ATTRS => {
            // Answer with what the job is really doing, so Get-Jobs and
            // Get-Job-Attributes can never disagree about the same job - a
            // client that sees "finished" from one and "still running" from
            // the other trusts neither and keeps asking.
            //
            // Only a job this process no longer holds (the app restarted
            // between submission and poll) keeps the old "already finished"
            // answer: an unknown id cannot be answered with not-found when a
            // client's local queue is waiting on it - that strands the queue.
            let requested = req_job_id(&attrs);
            let known = requested.and_then(|id| {
                let map = state.jobs.map.lock().unwrap();
                map.get(&id)
                    .map(|j| (rfc_job_state(j.ipp_state), j.info.name.clone()))
            });

            let id = requested.unwrap_or(1);
            let (job_state, name) =
                known.unwrap_or((RFC_JOB_STATE_COMPLETED, format!("Job {id}")));
            add_job_group(&mut resp, state, id, job_state);
            add_job_name(&mut resp, &name, "unknown");
        }

        _ => {
            resp.header_mut().operation_or_status =
                StatusCode::ServerErrorOperationNotSupported as u16;
        }
    }

    // Tell the client its document could not be accepted (RFC 8011 6.3) rather
    // than pretending success, and repeat the reason in `status-message` so it
    // also shows up in the Mac's own job history.
    if let Some(reason) = unprintable {
        resp.header_mut().operation_or_status =
            StatusCode::ClientErrorDocumentFormatNotSupported as u16;
        resp.attributes_mut().add(
            DelimiterTag::OperationAttributes,
            IppAttribute::new(
                "status-message".try_into().unwrap(),
                IppValue::TextWithoutLanguage(reason.as_str().try_into().unwrap()),
            ),
        );
        logln!("IPP -> client-error-document-format-not-supported (0x040A)");
    }

    Ok(resp.to_bytes().to_vec())
}

fn default_printer_uri(state: &IppState) -> String {
    format!("ipp://{}:{}/ipp/print", state.local_ip, state.port)
}

/// `printer-up-time` (RFC 8011 4.2.1) is REQUIRED in every printer attributes
/// group and must only ever climb, so it is anchored to process start rather
/// than to a broadcast cycle. A client that uses it to detect a restarted
/// endpoint rejects a response that leaves it out.
static SERVICE_STARTED_AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn service_up_time() -> i32 {
    let start = SERVICE_STARTED_AT.get_or_init(std::time::Instant::now);
    i32::try_from(start.elapsed().as_secs()).unwrap_or(i32::MAX)
}

/// One entry in the printer-attributes group of a response. Thin wrapper so
/// the job-template block below reads as data instead of as repeated
/// `DelimiterTag` / `IppAttribute::new` noise.
fn add_printer_attribute(attrs: &mut IppAttributes, name: &str, value: IppValue) {
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(name.try_into().unwrap(), value),
    );
}

fn keyword_list(values: &[&str]) -> IppValue {
    IppValue::Array(
        values
            .iter()
            .map(|v| IppValue::Keyword((*v).try_into().unwrap()))
            .collect(),
    )
}

fn enum_list(values: &[i32]) -> IppValue {
    IppValue::Array(values.iter().map(|v| IppValue::Enum(*v)).collect())
}

fn int_list(values: &[i32]) -> IppValue {
    IppValue::Array(values.iter().map(|v| IppValue::Integer(*v)).collect())
}

fn add_printer_attrs(resp: &mut IppRequestResponse, state: &IppState, printer_uri: &str) {
    let uuid_uri = format!("urn:uuid:{}", state.printer_uuid);
    let attrs = resp.attributes_mut();
    let name = &state.printer_name;

    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_URI_SUPPORTED.try_into().unwrap(),
            IppValue::Array(vec![IppValue::Uri(printer_uri.to_string().try_into().unwrap())]),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::URI_SECURITY_SUPPORTED.try_into().unwrap(),
            IppValue::Array(vec![IppValue::Keyword("none".try_into().unwrap())]),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::URI_AUTHENTICATION_SUPPORTED.try_into().unwrap(),
            IppValue::Array(vec![IppValue::Keyword("none".try_into().unwrap())]),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_NAME.try_into().unwrap(),
            IppValue::NameWithoutLanguage(name.as_str().try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "printer-dns-sd-name".try_into().unwrap(),
            IppValue::NameWithoutLanguage(name.as_str().try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_LOCATION.try_into().unwrap(),
            IppValue::TextWithoutLanguage("Local Network".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_INFO.try_into().unwrap(),
            IppValue::TextWithoutLanguage("NearbyPrint Virtual Printer".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_MAKE_AND_MODEL.try_into().unwrap(),
            IppValue::TextWithoutLanguage("NearbyPrint Virtual".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_MORE_INFO.try_into().unwrap(),
            IppValue::Uri(
                format!("http://{}:{}/", state.local_ip, state.port).try_into().unwrap(),
            ),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_STATE.try_into().unwrap(),
            IppValue::Enum(3),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_STATE_REASONS.try_into().unwrap(),
            IppValue::Keyword("none".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_IS_ACCEPTING_JOBS.try_into().unwrap(),
            IppValue::Boolean(true),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_UP_TIME.try_into().unwrap(),
            IppValue::Integer(service_up_time()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::IPP_VERSIONS_SUPPORTED.try_into().unwrap(),
            IppValue::Array(vec![
                IppValue::Keyword("1.1".try_into().unwrap()),
                IppValue::Keyword("2.0".try_into().unwrap()),
                IppValue::Keyword("2.1".try_into().unwrap()),
            ]),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::OPERATIONS_SUPPORTED.try_into().unwrap(),
            IppValue::Array(vec![
                IppValue::Enum(OP_PRINT_JOB as i32),
                IppValue::Enum(OP_VALIDATE_JOB as i32),
                IppValue::Enum(OP_CREATE_JOB as i32),
                IppValue::Enum(OP_SEND_DOCUMENT as i32),
                IppValue::Enum(OP_CANCEL_JOB as i32),
                IppValue::Enum(OP_GET_JOB_ATTRS as i32),
                IppValue::Enum(OP_GET_JOBS as i32),
                IppValue::Enum(OP_GET_PRINTER_ATTRS as i32),
                IppValue::Enum(OP_PAUSE_PRINTER as i32),
                IppValue::Enum(OP_RESUME_PRINTER as i32),
                IppValue::Enum(OP_PURGE_JOBS as i32),
            ]),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::CHARSET_CONFIGURED.try_into().unwrap(),
            IppValue::Charset("utf-8".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::CHARSET_SUPPORTED.try_into().unwrap(),
            IppValue::Charset("utf-8".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::NATURAL_LANGUAGE_CONFIGURED.try_into().unwrap(),
            IppValue::NaturalLanguage("en".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::GENERATED_NATURAL_LANGUAGE_SUPPORTED.try_into().unwrap(),
            IppValue::NaturalLanguage("en".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PDL_OVERRIDE_SUPPORTED.try_into().unwrap(),
            IppValue::Keyword("not-attempted".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::DOCUMENT_FORMAT_SUPPORTED.try_into().unwrap(),
            // Exactly two values, and nothing else: no `image/urf`, no
            // `image/pwg-raster`, no `application/postscript`, no `text/html`.
            //
            // This list is the handshake. The Windows IPP class driver reads it
            // from Get-Printer-Attributes and commits to a stream for every
            // job it will ever send us, so anything outside this pair is a
            // format we cannot render - and the local spooler then fails the
            // job with "Printing, Error" the moment it is submitted.
            //
            // * `application/pdf` - the default, and the only format the
            //   sidecar needs to be handed.
            // * `application/octet-stream` - the catch-all octet stream RFC
            //   8010 4.3.11 expects every IPP printer to accept, used when the
            //   client cannot pin the job to a typed format. The payload is
            //   identified by its magic bytes (`extension_for`), never by this
            //   label, so an octet-stream job that is really a PDF is still
            //   spooled as `.pdf`.
            IppValue::Array(vec![
                IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
                IppValue::MimeMediaType("image/urf".try_into().unwrap()),
                IppValue::MimeMediaType("application/octet-stream".try_into().unwrap()),
            ]),
        ),
    );
    add_printer_attribute(
        attrs,
        "urf-supported",
        keyword_list(&["V1.4", "W8", "SRGB24", "DM1"]),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::DOCUMENT_FORMAT_DEFAULT.try_into().unwrap(),
            IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::COLOR_SUPPORTED.try_into().unwrap(),
            IppValue::Boolean(true),
        ),
    );
    // RFC 8011 4.2.6: the jobs still running on this printer. A flat 0 next to
    // a Get-Jobs reply that carries real jobs reads as an inconsistent printer,
    // and a client that catches it in the act starts polling harder rather than
    // trusting either answer.
    let queued = {
        let map = state.jobs.map.lock().unwrap();
        map.values()
            .filter(|j| !is_terminal_job_state(rfc_job_state(j.ipp_state)))
            .count() as i32
    };
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::QUEUED_JOB_COUNT.try_into().unwrap(),
            IppValue::Integer(queued),
        ),
    );
    attrs.add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            IppAttribute::PRINTER_UUID.try_into().unwrap(),
            IppValue::Uri(uuid_uri.try_into().unwrap()),
        ),
    );

    // -----------------------------------------------------------------------
    // Job-template / media attributes.
    //
    // A Get-Printer-Attributes reply with only the RFC 8011 required set is
    // *valid*, but the Windows IPP class driver still refuses to submit the
    // job: it reads media-supported and the job-template defaults to build the
    // job ticket, and without them the spooler stalls after the capability
    // query and fails the job with "Printing, Error" - no Create-Job, no
    // Send-Document, nothing ever reaches this process. Verified live: a
    // stand-in server that answered with exactly this attribute set got the
    // driver all the way to Send-Document with the PDF payload.
    // -----------------------------------------------------------------------
    // Media: what the driver builds its job ticket from.
    //
    // Exactly two sizes, and A4 first: A4 and US Letter. Listing extra sizes
    // (legal, A5, B5, custom) only gave the Mac and Windows pickers options
    // this virtual printer cannot honour, and a client that defaulted to
    // `na_letter_8.5x11in` printed every A4 document scaled onto Letter paper.
    // `media-default` is A4 so both platforms open their print dialog on A4
    // unless the user changes it.
    add_printer_attribute(
        attrs,
        "media-supported",
        keyword_list(&["iso_a4_210x297mm", "na_letter_8.5x11in"]),
    );
    add_printer_attribute(attrs, "media-default", IppValue::Keyword("iso_a4_210x297mm".try_into().unwrap()));
    add_printer_attribute(
        attrs,
        "media-source-supported",
        keyword_list(&["auto", "main", "manual"]),
    );
    add_printer_attribute(attrs, "media-source-default", IppValue::Keyword("auto".try_into().unwrap()));
    add_printer_attribute(
        attrs,
        "media-type-supported",
        keyword_list(&["stationery"]),
    );
    add_printer_attribute(attrs, "media-type-default", IppValue::Keyword("stationery".try_into().unwrap()));
    for name in [
        "media-left-margin-supported",
        "media-right-margin-supported",
        "media-top-margin-supported",
        "media-bottom-margin-supported",
    ] {
        add_printer_attribute(attrs, name, IppValue::Integer(0));
    }
    // Resolution / quality / color.
    add_printer_attribute(
        attrs,
        "printer-resolution-default",
        IppValue::Resolution { cross_feed: 600, feed: 600, units: 3 },
    );
    add_printer_attribute(
        attrs,
        "printer-resolution-supported",
        IppValue::Array(vec![
            IppValue::Resolution { cross_feed: 600, feed: 600, units: 3 },
            IppValue::Resolution { cross_feed: 300, feed: 300, units: 3 },
            IppValue::Resolution { cross_feed: 1200, feed: 1200, units: 3 },
        ]),
    );
    add_printer_attribute(attrs, "print-quality-supported", enum_list(&[3, 4, 5]));
    add_printer_attribute(attrs, "print-quality-default", IppValue::Enum(4));
    add_printer_attribute(
        attrs,
        "print-color-mode-supported",
        keyword_list(&["auto", "color", "monochrome"]),
    );
    add_printer_attribute(attrs, "print-color-mode-default", IppValue::Keyword("auto".try_into().unwrap()));
    // Job-template defaults the driver reads before submitting.
    add_printer_attribute(attrs, "copies-default", IppValue::Integer(1));
    add_printer_attribute(
        attrs,
        "copies-supported",
        IppValue::RangeOfInteger { min: 1, max: 999 },
    );
    add_printer_attribute(
        attrs,
        "sides-supported",
        keyword_list(&["one-sided", "two-sided-long-edge", "two-sided-short-edge"]),
    );
    add_printer_attribute(attrs, "sides-default", IppValue::Keyword("one-sided".try_into().unwrap()));
    add_printer_attribute(attrs, "number-up-supported", int_list(&[1, 2, 4]));
    add_printer_attribute(attrs, "number-up-default", IppValue::Integer(1));
    add_printer_attribute(attrs, "orientation-requested-supported", enum_list(&[3, 4, 5, 6]));
    add_printer_attribute(attrs, "orientation-requested-default", IppValue::Enum(3));
    add_printer_attribute(attrs, "finishings-supported", enum_list(&[3]));
    add_printer_attribute(attrs, "finishings-default", IppValue::Enum(3));
    add_printer_attribute(attrs, "output-bin-supported", keyword_list(&["standard"]));
    add_printer_attribute(attrs, "output-bin-default", IppValue::Keyword("standard".try_into().unwrap()));
    add_printer_attribute(
        attrs,
        "print-scaling-supported",
        keyword_list(&["auto", "shrink-to-fit", "fit"]),
    );
    add_printer_attribute(attrs, "print-scaling-default", IppValue::Keyword("auto".try_into().unwrap()));
    add_printer_attribute(
        attrs,
        "pdf-fit-to-page-supported",
        IppValue::Array(vec![IppValue::Boolean(true), IppValue::Boolean(false)]),
    );
    add_printer_attribute(attrs, "pdf-fit-to-page-default", IppValue::Boolean(false));
    add_printer_attribute(attrs, "pages-per-minute", IppValue::Integer(10));
    add_printer_attribute(
        attrs,
        "mopria-certified",
        IppValue::TextWithoutLanguage("1.0".try_into().unwrap()),
    );
    add_printer_attribute(
        attrs,
        "client-info-supported",
        IppValue::Array(vec![IppValue::Boolean(false)]),
    );
    add_printer_attribute(
        attrs,
        "multiple-document-handling-supported",
        keyword_list(&[
            "separate-documents-collated-copies",
            "separate-documents-uncollated-copies",
        ]),
    );
    add_printer_attribute(
        attrs,
        "multiple-document-handling-default",
        IppValue::Keyword("separate-documents-collated-copies".try_into().unwrap()),
    );
    add_printer_attribute(attrs, "compression-supported", IppValue::Keyword("none".try_into().unwrap()));
}

// ---------------------------------------------------------------------------
// Document helpers
// ---------------------------------------------------------------------------

/// Extension for a spooled document, from the client-declared
/// `document-format` plus the bytes themselves.
///
/// Never returns `bin`: the silent printer only accepts `.pdf`, so anything
/// not positively recognised falls through to `pdf`.
fn extension_for(format: &str, data: &[u8]) -> &'static str {
    let f = format.to_lowercase();
    if data.starts_with(b"%PDF") {
        return "pdf";
    }
    // PostScript is checked before the generic paths: DSC comments can quote
    // "%PDF" in their own first bytes, and macOS queues installed with a
    // PostScript driver send these payloads as-is.
    if data.starts_with(b"%!") || f.contains("postscript") {
        return "ps";
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    if data.starts_with(b"RaS2") {
        return "pwg";
    }
    if data.starts_with(b"UNIRAST") {
        return "urf";
    }
    if f.contains("pwg-raster") {
        return "pwg";
    }
    if f.contains("urf") {
        return "urf";
    }
    if data.starts_with(b"\x89PNG") || f.contains("png") {
        return "png";
    }
    if (data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP") || f.contains("webp") {
        return "webp";
    }
    if f.contains("jpeg") || f.contains("jpg") {
        return "jpg";
    }
    "pdf"
}

// ---------------------------------------------------------------------------
// PostScript (macOS)
// ---------------------------------------------------------------------------

/// Shown on the job - and in the IPP `status-message` - when a PostScript
/// document cannot be converted, so the failure is actionable instead of a
/// bare "invalid argument" from the sidecar.
const POSTSCRIPT_HINT: &str = "This job arrived as PostScript, which cannot be printed directly. Install Ghostscript, or on the Mac remove this printer and re-add it with \"Secure AirPrint\" as the driver so it sends PDF.";

/// True for PostScript payloads: either the declared `document-format` or the
/// DSC header every PostScript file starts with.
fn is_postscript(format: &str, data: &[u8]) -> bool {
    data.starts_with(b"%!") || format.to_lowercase().contains("postscript")
}

/// Locates Ghostscript, the standard PostScript -> PDF converter. It is looked
/// up on `PATH` first and then in its usual install folders, because the
/// Windows installer does not add it to `PATH`.
fn find_ghostscript() -> Option<PathBuf> {
    const NAMES: [&str; 4] = ["gswin64c.exe", "gswin32c.exe", "gswin64c", "gswin32c"];

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in NAMES {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    for base in ["C:\\Program Files\\gs", "C:\\Program Files (x86)\\gs"] {
        let entries = match std::fs::read_dir(base) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            for name in NAMES {
                let candidate = version.join("bin").join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Renders a PostScript payload to PDF with Ghostscript, then cleans up the
/// scratch files. Returns `None` when Ghostscript is unavailable or fails.
fn postscript_to_pdf(data: &[u8]) -> Option<Vec<u8>> {
    let gs = find_ghostscript()?;
    let dir = job_dir();
    let stamp = format!(
        "nearby_print_{}_{}",
        Uuid::new_v4().simple(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let src = dir.join(format!("{stamp}.ps"));
    let dst = dir.join(format!("{stamp}.pdf"));
    std::fs::write(&src, data).ok()?;

    let args = vec![
        "-dNOPAUSE".to_string(),
        "-dBATCH".to_string(),
        "-dQUIET".to_string(),
        "-sDEVICE=pdfwrite".to_string(),
        "-o".to_string(),
        dst.to_string_lossy().to_string(),
        src.to_string_lossy().to_string(),
    ];
    let converted = run_hidden(&gs, &args, Duration::from_secs(120))
        .filter(|out| out.status.success())
        .is_some();
    let _ = std::fs::remove_file(&src);

    if !converted {
        let _ = std::fs::remove_file(&dst);
        logln!("Ghostscript could not convert PostScript ({})", gs.display());
        return None;
    }

    let pdf = std::fs::read(&dst).ok()?;
    let _ = std::fs::remove_file(&dst);
    if pdf.is_empty() {
        return None;
    }
    Some(pdf)
}

/// Makes a payload printable.
///
/// `Ok(Some(pdf))` only when a conversion actually happened (the caller then
/// swaps it in for the raw bytes), `Ok(None)` when the document needs no work,
/// and `Err(reason)` when it can never be printed - which the job arm reports
/// as an actionable job error rather than handing junk to the sidecar.
fn prepare_document(format: &str, data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if !is_postscript(format, data) {
        return Ok(None);
    }
    match postscript_to_pdf(data) {
        Some(pdf) => Ok(Some(pdf)),
        None => Err(POSTSCRIPT_HINT.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Service status
// ---------------------------------------------------------------------------

/// Reports whether the in-process `mdns-sd` daemon is up and answering.
///
/// This replaces the old `test_discovery` command, which shelled out to the
/// `dns-sd` CLI (a macOS/Bonjour tool that does not exist on Windows).
#[tauri::command]
fn check_mdns_status(state: tauri::State<'_, AppState>) -> bool {
    // A healthy daemon replies over its command channel in microseconds; the
    // 500ms cap only applies if the daemon thread itself has wedged. A dead
    // thread is detected immediately (disconnected channel).
    match state.mdns.lock() {
        Ok(guard) => guard.as_ref().map_or(false, mdns_daemon_is_running),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Windows Firewall
// ---------------------------------------------------------------------------

/// Prefix shared by every rule this app owns so they can be replaced cleanly.
#[cfg(windows)]
const FIREWALL_RULE_PREFIX: &str = "NearbyPrint";

/// Makes sure inbound traffic is allowed for this executable:
///
/// * `TCP 8631` — the IPP printer endpoint
/// * `UDP 5353` — mDNS discovery of `_ipp._tcp.local.`
///
/// Windows Defender Firewall drops both by default, so without these rules the
/// service registers fine but no phone can reach it.
///
/// `netsh advfirewall` needs administrator rights. When the process is not
/// elevated the command fails with "Access is denied"; that is swallowed here
/// and `false` is returned so the caller can log a hint instead of crashing.
///
/// Existing rules with the same names are deleted first, which keeps the run
/// idempotent (no duplicates piling up across restarts) and repairs the rule
/// after the `.exe` has been moved or updated.
#[cfg(windows)]
fn ensure_windows_firewall_rules() -> bool {
    use std::os::windows::process::CommandExt;

    // CREATE_NO_WINDOW: never flash a console window from the GUI process.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let _exe = match std::env::current_exe() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(e) => {
            logln!("firewall: cannot resolve the running .exe path: {e}");
            return false;
        }
    };

    let rules: [(&str, &str); 2] = [("TCP", "8631"), ("UDP", "5353")];
    let mut all_ok = true;

    for (protocol, port) in rules {
        let name = format!("{FIREWALL_RULE_PREFIX} - allow {protocol} {port}");

        // Best effort cleanup: silently fails when the rule does not exist
        // yet or when we are not elevated.
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                &format!("name={name}"),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        let result = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={name}"),
                "dir=in",
                "action=allow",
                &format!("protocol={protocol}"),
                &format!("localport={port}"),
                "profile=any",
                "enable=yes",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        match result {
            Ok(output) if output.status.success() => {
                logln!("firewall: rule '{name}' installed (any program)");
            }
            Ok(output) => {
                all_ok = false;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = if stderr.trim().is_empty() {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                } else {
                    stderr.trim().to_string()
                };
                logln!("firewall: could not add '{name}': {detail}");
            }
            Err(e) => {
                all_ok = false;
                logln!("firewall: cannot run netsh: {e}");
            }
        }
    }

    all_ok
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn ensure_windows_firewall_rules() -> bool {
    true
}

// ---------------------------------------------------------------------------

fn main() {
    #[cfg(all(windows, debug_assertions))]
    {
        if std::env::var("WEBVIEW2_USER_DATA_FOLDER").is_err() {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                let dev_dir = std::path::PathBuf::from(local_app_data)
                    .join("com.nearbyprint.app-dev")
                    .join("EBWebView");
                let _ = std::fs::create_dir_all(&dev_dir);
                std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dev_dir);
            }
        }
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            // Windows blocks unsolicited inbound UDP 5353 / TCP 8631, so open
            // them for our own executable before we start broadcasting.
            #[cfg(windows)]
            {
                if !ensure_windows_firewall_rules() {
                    logln!(
                        "firewall: rules were not installed (administrator rights required). \
                         Run the app once as Administrator, otherwise other devices will not \
                         be able to discover or reach this printer."
                    );
                }
            }

            // Restore whatever the user last chose. Windows keeps a queue
            // pointed at 8631, so a launch that stays silent puts that queue
            // straight back into Error before the user has touched anything.
            let handle = app.handle().clone();

            // The name comes back from disk before anything that reads it:
            // the auto-restore below registers mDNS with it, and the IPP
            // responses are built from it, so loading it later would publish
            // one name and answer with another.
            if let Some(saved) = load_printer_name(&handle) {
                *handle.state::<AppState>().printer_name.lock().unwrap() = saved;
            }

            // The two settings that decide what this process does with a
            // document it receives: where to forward it, and which local queue
            // a forwarded document is printed on. Loaded before the server
            // starts, because the very first request that arrives has to be
            // answered with the right role.
            *handle.state::<AppState>().relay_host.lock().unwrap() =
                read_config(&handle, RELAY_HOST_FILE).unwrap_or_default();
            *handle.state::<AppState>().selected_printer.lock().unwrap() =
                read_config(&handle, SELECTED_PRINTER_FILE).unwrap_or_default();
            let relay_configured = !handle
                .state::<AppState>()
                .relay_host
                .lock()
                .unwrap()
                .trim()
                .is_empty();

            // Which role this process came up in is the first thing anyone
            // debugging a print needs to know, and it is invisible in the UI
            // once the server is up.
            match relay_url(
                &handle
                    .state::<AppState>()
                    .relay_host
                    .lock()
                    .unwrap()
                    .clone(),
            ) {
                Ok(url) => logln!("Relay: forwarding received documents to {url}"),
                Err(e) if relay_configured => {
                    logln!("Relay: a host is configured but unusable - {e}")
                }
                Err(_) => logln!("Relay: off - this PC prints what it receives itself"),
            }
            let print_on = handle.state::<AppState>().selected_printer.lock().unwrap().clone();
            if !print_on.trim().is_empty() {
                logln!("Relay: forwarded jobs print on '{print_on}'");
            }

            // Two things come up on every launch, whatever else this PC does:
            //
            // * the loopback proxy the virtual printer points at, so a queue
            //   installed last week finds a server the moment the app starts -
            //   and so installing one never depends on the broadcast toggle;
            // * the mDNS browse for other hosts, which is what lets "Print
            //   from this PC" offer a list instead of an address field.
            let proxy_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                match start_local_proxy(&proxy_handle).await {
                    Ok(url) => logln!("Local IPP proxy listening on {url}"),
                    Err(e) => logln!("Local IPP proxy did not start: {e}"),
                }
            });
            if let Err(e) = start_discovery(handle.clone()) {
                logln!("Host discovery did not start: {e}");
            }

            // A client PC with a relay configured still brings the network
            // server up. The virtual printer does not need it any more - that
            // is the loopback proxy's job now - but a queue installed by an
            // earlier build points at *this* listener, and leaving it dead is
            // exactly the silent breakage this restore exists to prevent.
            // Restore local wireless IPP + mDNS printer broadcasting at startup
            // so phones, Macs, and PCs on the same Wi-Fi immediately find NearbyPrint.
            if broadcast_preference(&handle) || relay_configured || true {
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    match toggle_broadcast(handle.clone(), state, true).await {
                        Ok(url) => logln!("Broadcasting restored: {url}"),
                        Err(e) => logln!("Could not restore broadcasting: {e}"),
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_printers,
            get_available_printers,
            get_local_ips,
            get_network_details,
            check_firewall_status,
            fix_windows_firewall,
            get_printer_name,
            update_printer_name,
            toggle_broadcast,
            start_server,
            stop_server,
            get_broadcast_status,
            get_server_status,
            add_windows_printer,
            install_virtual_printer,
            get_relay_status,
            set_relay_host,
            get_selected_printer,
            set_selected_printer,
            print_file,
            cancel_job,
            submit_cloud_job,
            check_mdns_status,
            start_discovery,
            get_discovered_hosts,
            refresh_discovery,
            get_local_proxy_status,
            generate_client_script
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            shutdown_everything(handle);
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const SAMPLE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]>>endobj\n\
trailer<</Root 1 0 R>>\n\
%%EOF\n";

    fn test_state() -> IppState {
        IppState {
            app: None,
            jobs: Arc::new(Jobs::default()),
            local_ip: "192.168.1.5".into(),
            // The network port by default: every test below answers as the
            // broadcast server does. The loopback proxy is the same state
            // with `LOCAL_PROXY_PORT` in this slot.
            port: IPP_PORT,
            printer_uuid: "fd6890d8-0000-4000-8000-000000000000".into(),
            printer_name: "Test Printer".into(),
            // Host role by default: nothing is configured to forward to, so
            // every test below exercises the local path unchanged.
            relay_host: Arc::new(Mutex::new(String::new())),
            selected_printer: Arc::new(Mutex::new(String::new())),
            relay_warning: Arc::new(Mutex::new(String::new())),
        }
    }

    fn peer() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 40123))
    }

    /// Stands up the real HTTP+IPP server on the real port so a *real* client
    /// (Windows' Microsoft IPP Class Driver, an iPhone, macOS) can be pointed
    /// at it. Ignored by default: it binds 8631 and never returns.
    ///
    /// ```text
    /// cargo test -- --ignored live_ipp_server --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_ipp_server_for_manual_verification() {
        let state = IppState {
            app: None,
            jobs: Arc::new(Jobs::default()),
            local_ip: "192.168.1.6".into(),
            port: IPP_PORT,
            // The uuid Windows recorded when it installed this queue. The app
            // mints a fresh uuid on every broadcast, which is exactly what
            // leaves a cached Windows queue pointing at a device that no
            // longer identifies itself the same way.
            printer_uuid: "0f65213b-14fd-4588-86a9-b23f054bb73e".into(),
            printer_name: "NearbyPrint - Nootan".into(),
            relay_host: Arc::new(Mutex::new(String::new())),
            selected_printer: Arc::new(Mutex::new(String::new())),
            relay_warning: Arc::new(Mutex::new(String::new())),
        };
        let router = Router::new()
            .route("/", get(ipp_get).post(ipp_post))
            .route("/ipp/print", get(ipp_get).post(ipp_post))
            .route(RELAY_PATH, post(relay_print))
            .fallback(get(ipp_get).post(ipp_post))
            .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
            .with_state(Arc::new(state));

        let addr = SocketAddr::from(([0, 0, 0, 0], IPP_PORT));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind(addr))
            .expect("could not bind 8631 - is the app already broadcasting?");
        println!(
            "LIVE IPP server on {addr} - clients should POST to http://192.168.1.6:{IPP_PORT}/ipp/print"
        );
        rt.block_on(async {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("IPP server stopped");
        });
    }

    fn request(op: Operation, req_id: u32, extra: &[(&str, IppValue)], payload: &[u8]) -> Vec<u8> {
        let uri: Uri = "ipp://192.168.1.5:8631/ipp/print".parse().unwrap();
        let mut req = IppRequestResponse::new(IppVersion::v1_1(), op, Some(uri)).unwrap();
        req.header_mut().request_id = req_id;
        for (name, value) in extra {
            req.attributes_mut().add(
                DelimiterTag::OperationAttributes,
                IppAttribute::new((*name).try_into().unwrap(), value.clone()),
            );
        }
        let mut bytes = req.to_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    fn parse(body: Vec<u8>) -> (IppHeader, IppAttributes) {
        let (header, attrs, _reader) =
            IppParser::new(std::io::Cursor::new(body)).parse_parts().unwrap();
        (header, attrs)
    }

    fn exchange(state: &IppState, req: Vec<u8>) -> (IppHeader, IppAttributes) {
        parse(
            build_response(state, &req, peer())
                .expect("IPP request was rejected before a response could be built"),
        )
    }

    fn int_attr(attrs: &IppAttributes, name: &str) -> Option<i32> {
        req_int(attrs, name)
    }

    fn list_attr(attrs: &IppAttributes, name: &str) -> Vec<String> {
        match req_attr(attrs, name) {
            Some(IppValue::Array(items)) => items.iter().filter_map(value_to_string).collect(),
            Some(other) => value_to_string(other).into_iter().collect(),
            None => Vec::new(),
        }
    }

    #[test]
    fn printer_attributes_advertise_exactly_pdf_and_octet_stream() {
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::GetPrinterAttributes, 1, &[], &[]));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(int_attr(&attrs, "printer-state"), Some(3));
        assert_eq!(req_str(&attrs, "printer-name").as_deref(), Some("Test Printer"));

        // The handshake every Windows job is built from: exactly these two
        // formats and nothing else. A third entry - a raster, PostScript,
        // text/html - is a stream we cannot render, and the client that picks
        // it fails the job with "Printing, Error" as soon as it submits.
        let formats = list_attr(&attrs, "document-format-supported");
        assert_eq!(
            formats,
            vec![
                "application/pdf".to_string(),
                "application/octet-stream".to_string(),
            ]
        );
        // The default stays the format we can actually hand to the sidecar.
        assert_eq!(
            req_str(&attrs, "document-format-default").as_deref(),
            Some("application/pdf")
        );
        for banned in ["image/urf", "image/pwg-raster", "application/postscript", "text/html"] {
            assert!(
                !formats.iter().any(|f| f == banned),
                "{banned} must never be advertised"
            );
        }

        // The mDNS `pdl` record has to promise no more than the IPP handshake
        // does: a discovery client reads it *before* it ever asks us.
        let pdl = txt_properties()
            .iter()
            .find(|(key, _)| *key == "pdl")
            .map(|(_, value)| *value)
            .expect("pdl must be advertised");
        // `application/pdf` is the entry AirPrint discovery requires before a
        // Mac/phone will offer this printer at all; `image/urf` is the
        // spelling those clients match on. Nothing else may appear: a PDL we
        // cannot render (PostScript, PWG raster) would be picked by a client
        // and fail the job on submit.
        assert_eq!(pdl, "application/pdf,image/urf");
        assert!(
            pdl.split(',').any(|p| p == "application/pdf"),
            "AirPrint discovery looks for application/pdf in pdl"
        );
        for banned in ["application/postscript", "image/pwg-raster", "text/html"] {
            assert!(
                !pdl.split(',').any(|p| p == banned),
                "{banned} must never be promised in pdl"
            );
        }
        // `URF=none` is what stops a client that saw `image/urf` above from
        // actually *sending* URF raster: it declares the raster path
        // unsupported, so the client falls back to PDF.
        assert_eq!(
            txt_properties()
                .iter()
                .find(|(key, _)| *key == "URF")
                .map(|(_, value)| *value),
            Some("none")
        );
    }

    #[test]
    fn printer_attributes_include_the_rfc_8011_required_set() {
        // RFC 8011 4.2.1 marks these REQUIRED in every printer attributes
        // group. A client that validates the group drops the printer instead of
        // printing to it - on Windows that is a queue pinned in Error while its
        // status polls against the same endpoint keep succeeding.
        let state = test_state();
        let (_, attrs) = exchange(&state, request(Operation::GetPrinterAttributes, 1, &[], &[]));

        for name in [
            "printer-uri-supported",
            "uri-authentication-supported",
            "uri-security-supported",
            "printer-state",
            "printer-state-reasons",
            "printer-is-accepting-jobs",
            "printer-up-time",
            "queued-job-count",
            "ipp-versions-supported",
            "operations-supported",
            "charset-configured",
            "charset-supported",
            "natural-language-configured",
            "generated-natural-language-supported",
            "document-format-default",
            "document-format-supported",
            "pdl-override-supported",
        ] {
            assert!(
                req_attr(&attrs, name).is_some(),
                "RFC 8011 4.2.1 requires {name}"
            );
        }

        let up_time =
            int_attr(&attrs, "printer-up-time").expect("printer-up-time must be present");
        assert!(
            up_time >= 0,
            "printer-up-time counts elapsed seconds, so it cannot be negative"
        );

        // It is a *monotonic* counter: a later response may never report less.
        let (_, later) = exchange(&state, request(Operation::GetPrinterAttributes, 2, &[], &[]));
        assert!(int_attr(&later, "printer-up-time").unwrap() >= up_time);
    }

    /// The regression that kept the Windows queue pinned in "Printing, Error":
    /// a *valid* Get-Printer-Attributes reply (RFC 8011 required set complete)
    /// that still omitted every job-template/media attribute. The IPP class
    /// driver builds its job ticket from these; without them it queried the
    /// printer, never sent Create-Job/Send-Document, and the spooler failed
    /// the job. A stand-in server advertising exactly this set reached
    /// Send-Document with the full PDF payload - so the set below is what the
    /// real reply has to carry.
    #[test]
    fn printer_attributes_include_the_job_template_the_windows_driver_needs() {
        let state = test_state();
        let (_, attrs) = exchange(&state, request(Operation::GetPrinterAttributes, 1, &[], &[]));

        for name in [
            // Media - the job ticket.
            "media-supported",
            "media-default",
            "media-source-supported",
            "media-source-default",
            "media-type-supported",
            "media-type-default",
            "media-left-margin-supported",
            "media-right-margin-supported",
            "media-top-margin-supported",
            "media-bottom-margin-supported",
            // Resolution / quality / color.
            "printer-resolution-default",
            "printer-resolution-supported",
            "print-quality-supported",
            "print-quality-default",
            "print-color-mode-supported",
            "print-color-mode-default",
            // Job-template defaults.
            "copies-default",
            "copies-supported",
            "sides-supported",
            "sides-default",
            "number-up-supported",
            "number-up-default",
            "orientation-requested-supported",
            "orientation-requested-default",
            "finishings-supported",
            "finishings-default",
            "output-bin-supported",
            "output-bin-default",
            "print-scaling-supported",
            "print-scaling-default",
            "pdf-fit-to-page-supported",
            "pdf-fit-to-page-default",
            "pages-per-minute",
            "mopria-certified",
            "client-info-supported",
            "multiple-document-handling-supported",
            "multiple-document-handling-default",
            "compression-supported",
        ] {
            assert!(
                req_attr(&attrs, name).is_some(),
                "the Windows driver reads {name} before it submits a job"
            );
        }

        // Values that a driver rejects outright if they are wrong-shaped.
        // Exactly these two, A4 first: anything more (legal, A5, B5, custom)
        // is a size this virtual printer cannot honour, and a client that
        // defaulted to Letter printed every A4 document scaled down.
        let media = list_attr(&attrs, "media-supported");
        assert_eq!(
            media,
            vec![
                "iso_a4_210x297mm".to_string(),
                "na_letter_8.5x11in".to_string(),
            ],
            "only A4 and US Letter may be offered"
        );
        assert_eq!(
            req_str(&attrs, "media-default").as_deref(),
            Some("iso_a4_210x297mm")
        );
        // The four orientations the dialog can pick, and the portrait default
        // (RFC 8011 4.2.14: 3 = portrait, 4 = landscape, 5 = reverse
        // landscape, 6 = reverse portrait).
        let orientations: Vec<i32> = match req_attr(&attrs, "orientation-requested-supported") {
            Some(IppValue::Array(items)) => items
                .iter()
                .filter_map(|v| match v {
                    IppValue::Enum(i) | IppValue::Integer(i) => Some(*i),
                    _ => None,
                })
                .collect(),
            other => panic!("orientation-requested-supported must be an enum array, got {other:?}"),
        };
        assert_eq!(orientations, vec![3, 4, 5, 6]);
        assert_eq!(int_attr(&attrs, "orientation-requested-default"), Some(3));
        assert!(
            matches!(
                req_attr(&attrs, "printer-resolution-default"),
                Some(IppValue::Resolution { cross_feed: 600, feed: 600, .. })
            ),
            "printer-resolution-default must keep its resolution syntax"
        );
        assert!(
            matches!(
                req_attr(&attrs, "copies-supported"),
                Some(IppValue::RangeOfInteger { min: 1, max: 999 })
            ),
            "copies-supported must keep its range syntax"
        );
        assert!(
            matches!(req_attr(&attrs, "print-quality-default"), Some(IppValue::Enum(4))),
            "print-quality-default must stay an enum, not an integer"
        );
        assert!(int_attr(&attrs, "pages-per-minute").is_some());
        // Sanity: the RFC required set must not have been displaced.
        for name in ["document-format-supported", "operations-supported", "printer-state"] {
            assert!(req_attr(&attrs, name).is_some(), "{name} must not be lost");
        }
    }

    /// Two NearbyPrint PCs used to answer mDNS with the *same* SRV target
    /// (`NearbyPrint.local.`), so both instances resolved to whichever IP the
    /// querier happened to see first - the device list then showed the right
    /// name against the wrong address, or the same machine twice.
    #[test]
    fn mdns_srv_target_is_unique_per_machine() {
        let host = mdns_server_hostname();
        assert!(
            host.ends_with(".local."),
            "SRV targets must be absolute, got {host}"
        );
        let label = host.strip_suffix(".local.").unwrap();
        assert!(!label.is_empty(), "hostname label must not be empty");
        assert_ne!(
            label, "nearbyprint",
            "a shared label is exactly the collision that made two PCs resolve to one IP"
        );
        assert!(
            label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "mDNS labels are DNS labels: [a-z0-9-] only, got {label}"
        );
        assert!(
            !label.starts_with('-') && !label.ends_with('-'),
            "DNS labels may not start or end with '-', got {label}"
        );
        // Derived from this machine's name, so it is stable across restarts.
        assert_eq!(host, mdns_server_hostname());
    }

    #[test]
    fn request_id_is_echoed_back() {
        let state = test_state();
        let (header, _) = exchange(&state, request(Operation::GetPrinterAttributes, 773, &[], &[]));
        assert_eq!(header.request_id, 773);
    }

    #[test]
    fn print_job_returns_a_job_group_the_client_can_poll() {
        // A Windows spooler will not clear the local job until the response
        // carries an id it can address, a state, and a URI it can poll.
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::PrintJob, 2, &[("document-format", IppValue::MimeMediaType("application/pdf".try_into().unwrap()))], SAMPLE_PDF));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        let job_id = int_attr(&attrs, "job-id").expect("job-id must be present");
        // 5 = processing: the bytes were accepted and the job has entered the
        // print pipeline. Claiming 9 (completed) here is the lie that made the
        // Mac report "Unable to communicate with the printer" - it stopped
        // polling the moment we said finished, while the job was still being
        // spooled.
        assert_eq!(int_attr(&attrs, "job-state"), Some(5));
        assert!(
            matches!(req_attr(&attrs, "job-state"), Some(IppValue::Enum(_))),
            "job-state must keep its enum syntax"
        );
        let job_uri =
            req_str(&attrs, "job-uri").expect("job-uri must be present");
        assert_eq!(
            job_uri,
            format!("ipp://192.168.1.5:{IPP_PORT}/jobs/{job_id}"),
            "job-uri must address this server, not the client's loopback"
        );
        assert!(!job_uri.contains("localhost"));
        // RFC 8011 4.1.4: charset and natural language lead every response's
        // operation attributes - a spooler that cannot read the rest of the
        // message treats the submission as failed.
        assert!(req_str(&attrs, "attributes-charset").is_some());
        assert!(req_str(&attrs, "attributes-natural-language").is_some());
        assert_eq!(
            req_str(&attrs, "job-state-reasons").as_deref(),
            Some("none"),
            "job-state-reasons is what tells the spooler the job was accepted"
        );
        if let Some(p) = state.jobs.take_path(job_id as u32) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn send_document_returns_a_job_group_the_client_can_poll() {
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::SendDocument, 3, &[("job-id", IppValue::Integer(1)), ("last-document", IppValue::Boolean(true)), ("document-format", IppValue::MimeMediaType("application/pdf".try_into().unwrap()))], SAMPLE_PDF));
        // Same contract as Print-Job: a success status the spooler can read,
        // plus the three mandatory job attributes (RFC 8011 4.3.4).
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert!(req_str(&attrs, "attributes-charset").is_some());
        assert!(req_str(&attrs, "attributes-natural-language").is_some());
        assert!(int_attr(&attrs, "job-id").is_some(), "job-id must be present");
        assert!(req_str(&attrs, "job-uri").is_some(), "job-uri must be present");
        // 5 = processing: accepted and printing, not "already finished" the
        // instant the last document arrived.
        assert_eq!(int_attr(&attrs, "job-state"), Some(5));
        assert!(
            matches!(req_attr(&attrs, "job-state"), Some(IppValue::Enum(_))),
            "job-state must keep its enum syntax"
        );
        assert_eq!(req_str(&attrs, "job-state-reasons").as_deref(), Some("none"));
        if let Some(p) = state.jobs.take_path(1) {
            let _ = std::fs::remove_file(p);
        }
    }

    /// The label Windows now picks, since `application/octet-stream` is part
    /// of the advertised set: the format is *not* trusted, the payload's magic
    /// bytes decide what gets spooled, and the response is a full success -
    /// not `client-error-document-format-not-supported`, which is what puts
    /// the job into "Printing, Error" on the client.
    #[test]
    fn octet_stream_jobs_are_accepted_and_spooled_as_pdf() {
        let state = test_state();
        let (header, attrs) = exchange(
            &state,
            request(
                Operation::PrintJob,
                77,
                &[(
                    "document-format",
                    IppValue::MimeMediaType("application/octet-stream".try_into().unwrap()),
                )],
                SAMPLE_PDF,
            ),
        );
        assert_eq!(
            header.operation_or_status,
            StatusCode::SuccessfulOk as u16,
            "an advertised format must never be answered with an error status"
        );
        let job_id = int_attr(&attrs, "job-id").expect("job-id must be present");
        assert!(req_str(&attrs, "job-uri").is_some(), "job-uri must be present");
        // 5 = processing: accepted and spooling. The client keeps polling
        // Get-Job-Attributes for the real 9 once the engine is done.
        assert_eq!(int_attr(&attrs, "job-state"), Some(5));

        let info = state
            .jobs
            .get_info(job_id as u32)
            .expect("the job must be stored");
        let path = info.file_path.clone().unwrap_or_default();
        assert!(
            path.ends_with(".pdf"),
            "an octet-stream job holding PDF bytes must be spooled as .pdf, got {path}"
        );
        assert_eq!(info.status, "pending");
        if let Some(p) = state.jobs.take_path(job_id as u32) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn send_document_echoes_a_job_id_it_never_handed_out() {
        // The spooler addresses its job by the id it was given. Answering with
        // a freshly minted one makes it poll a job that exists on neither side.
        let state = test_state();
        let (header, attrs) = exchange(
            &state,
            request(
                Operation::SendDocument,
                4,
                &[
                    ("job-id", IppValue::Integer(4242)),
                    ("last-document", IppValue::Boolean(true)),
                    ("document-format", IppValue::MimeMediaType("application/pdf".try_into().unwrap())),
                ],
                SAMPLE_PDF,
            ),
        );
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(int_attr(&attrs, "job-id"), Some(4242));
        // 5 = processing: the document was accepted and is being printed.
        assert_eq!(int_attr(&attrs, "job-state"), Some(5));
        assert!(req_str(&attrs, "job-uri").is_some(), "job-uri must be present");
        if let Some(p) = state.jobs.take_path(4242) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn create_job_returns_mandatory_job_attributes() {
        // macOS opens a job with Create-Job (0x0005) before it sends a single
        // byte. Without job-id / job-uri / job-state the client has nothing to
        // address the follow-up Send-Document with, and the job never lands.
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::CreateJob, 8, &[], &[]));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert!(int_attr(&attrs, "job-id").is_some(), "job-id must be present");
        assert!(req_str(&attrs, "job-uri").is_some(), "job-uri must be present");
        // 3 = processing: the job is open and waiting for its document.
        assert_eq!(int_attr(&attrs, "job-state"), Some(3));
        assert!(state.jobs.get_info(int_attr(&attrs, "job-id").unwrap() as u32).is_some());
    }

    #[test]
    fn send_document_attaches_to_the_created_job() {
        let state = test_state();
        let (_, created) = exchange(&state, request(Operation::CreateJob, 9, &[], &[]));
        let job_id = int_attr(&created, "job-id").expect("Create-Job must hand out a job-id");

        let (header, attrs) = exchange(
            &state,
            request(
                Operation::SendDocument,
                10,
                &[
                    ("job-id", IppValue::Integer(job_id)),
                    ("last-document", IppValue::Boolean(true)),
                    ("document-format", IppValue::MimeMediaType("application/pdf".try_into().unwrap())),
                ],
                SAMPLE_PDF,
            ),
        );
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(
            int_attr(&attrs, "job-id"),
            Some(job_id),
            "Send-Document must stay on the job Create-Job opened"
        );
        // 5 = processing, no longer the 3 Create-Job opened the job with: the
        // document has landed and the engine has it.
        assert_eq!(int_attr(&attrs, "job-state"), Some(5));

        let info = state.jobs.get_info(job_id as u32).expect("job must be registered");
        assert_eq!(info.bytes, SAMPLE_PDF.len() as u64, "payload must be stored");
        assert!(info.file_path.is_some(), "document must be on disk");
        if let Some(p) = state.jobs.take_path(job_id as u32) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn win32_printer_status_decides_online() {
        // Field values exactly as `Get-CimInstance Win32_Printer` reports them.
        assert!(is_printer_online(Some(false), Some(3)), "idle");
        assert!(is_printer_online(Some(false), Some(4)), "printing");
        assert!(is_printer_online(Some(false), Some(5)), "warming up");
        assert!(!is_printer_online(Some(true), Some(3)), "Use Printer Offline");
        assert!(!is_printer_online(None, Some(7)), "hardware off / unreachable");
        assert!(!is_printer_online(None, Some(6)), "stopped printing");
        assert!(is_printer_online(None, None), "unknown must never hide a queue");
    }

    #[test]
    fn printer_status_json_is_parsed() {
        let json = r#"[{"Name":"HP LaserJet (Home)","WorkOffline":true,"PrinterStatus":3},{"Name":"Canon G2010 series","WorkOffline":false,"PrinterStatus":3}]"#;
        let map = parse_printer_statuses(json);
        assert_eq!(map.get("hp laserjet (home)"), Some(&false));
        assert_eq!(map.get("canon g2010 series"), Some(&true));

        // PowerShell emits a bare object when exactly one printer is installed.
        let single = r#"{"Name":"Only One","WorkOffline":false,"PrinterStatus":3}"#;
        assert_eq!(parse_printer_statuses(single).get("only one"), Some(&true));

        assert!(parse_printer_statuses("").is_empty());
        assert!(parse_printer_statuses("not json").is_empty());
    }

    #[test]
    fn sidecar_arguments_target_the_selected_printer() {
        // `-exit-when-done` is not optional: without it SumatraPDF stays open
        // and the caller waits forever on a process that never returns.
        let args = sidecar_args(
            "HP LaserJet (Home)",
            Path::new(r"C:\jobs\doc.pdf"),
            PrintOptions::default(),
        );
        assert_eq!(
            args,
            vec![
                "-print-to",
                "HP LaserJet (Home)",
                "-silent",
                "-exit-when-done",
                r"C:\jobs\doc.pdf"
            ]
        );
    }

    #[test]
    fn sidecar_falls_back_to_the_default_printer_when_none_is_named() {
        let args = sidecar_args("   ", Path::new(r"C:\jobs\doc.pdf"), PrintOptions::default());
        assert_eq!(
            args,
            vec![
                "-print-to-default",
                "-silent",
                "-exit-when-done",
                r"C:\jobs\doc.pdf"
            ]
        );
    }

    #[test]
    fn sidecar_binary_is_the_bundled_sumatrapdf() {
        // Renaming the externalBin without updating this constant would leave
        // the app looking for a tool that no longer exists and silently
        // falling back to the interactive shell print.
        assert_eq!(SIDECAR_EXE, "SumatraPDF.exe");
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("bin")
            .join(format!(
                "{}-x86_64-pc-windows-msvc.exe",
                SIDECAR_EXE.trim_end_matches(".exe")
            ));
        assert!(
            bundled.is_file(),
            "{} must be present (tauri-build refuses to package without it)",
            bundled.display()
        );
    }

    #[test]
    fn sumatrapdf_flag_chatter_is_never_reported_as_the_reason() {
        // Verbatim output of SumatraPDF 3.6 on a run that printed nothing: the
        // flag dump is printed on success too, so a failure screen made of it
        // told the user nothing about the printer and pointed at `-print-to`.
        let chatter = "ParseFlags: argName: '-print-to', arg: 33\n\
                       ParseFlags: argName: '-silent', arg: 1\n\
                       ParseFlags: argName: '-exit-when-done', arg: 8\n";
        assert_eq!(sidecar_failure_lines(chatter), "");

        // Anything the printer itself says still reaches the job message.
        let complaint = "ParseFlags: argName: '-silent', arg: 1\nprinter is out of paper\n";
        assert_eq!(sidecar_failure_lines(complaint), "printer is out of paper");
    }

    #[test]
    fn client_script_targets_this_server_with_the_ip_driver() {
        let script = powershell_setup_script("192.168.1.5", "NearbyPrint - Office");
        assert!(script.contains("http://192.168.1.5:8631/ipp/print"));
        assert!(script.contains("'192.168.1.5'"));
        // Both install paths: the adaptive IPP port from a URL, and the
        // no-user-interaction printui fallback for builds without -IppURL.
        assert!(script.contains("-IppURL"));
        assert!(script.contains("'printui.dll,PrintUIEntry'"));
        // The driver is never left to the user to find in a list.
        assert!(script.contains("Microsoft IPP Class Driver"));
        // `Add-PrinterPort` can only make local / LPR / Standard TCP/IP ports,
        // and a RAW TCP port pointed at an HTTP server installs happily while
        // never delivering a job. It must stay out of the shipped script.
        assert!(!script.contains("Add-PrinterPort"));
        // Self-elevation: both paths need administrator, and a client user
        // who has to discover that themselves will just give up.
        assert!(script.contains("RunAs"));
        // Reachability is checked before anything is installed, so a stopped
        // broadcast fails with instructions instead of a spooler error.
        assert!(script.contains("BeginConnect($Server, 8631"));
        // Idempotent: a second run must not fight over an existing queue.
        assert!(script.contains("Get-Printer -Name $Name -ErrorAction SilentlyContinue"));
        // Every token must have been substituted - a leftover `{{...}}` in a
        // shipped script is a hard failure on the client.
        assert!(!script.contains("{{"));
    }

    #[test]
    fn client_script_escapes_a_printer_name_that_contains_a_quote() {
        let script = powershell_setup_script("192.168.1.5", "Bob's Printer");
        assert!(script.contains("$Name   = 'Bob''s Printer'"));
        assert!(!script.contains("{{PRINTER_NAME}}"));
    }

    #[test]
    fn client_script_never_leaves_the_ipp_port_out() {
        let script = powershell_setup_script("10.0.0.7", "NearbyPrint");
        // The full URL is the one and only endpoint the script ever hands
        // out, to both install paths.
        assert!(script.contains("$Url    = 'http://10.0.0.7:8631/ipp/print'"));
        // The manual escape hatch prints that same URL instead of a bare
        // host:port pair, which would install a TCP port that cannot connect.
        assert!(script.contains("'    ' + $Url"));
        // Nothing may fall back to the CUPS default port (631) - `:8631` is
        // ours, and `:631/ipp` would only match a different port entirely.
        assert!(!script.contains(":631/ipp/print"));
    }

    #[test]
    fn client_script_parses_as_valid_powershell() {
        // A syntax error here is invisible until someone else runs the script
        // on their own machine, by which point it is their problem. Parse it
        // with PowerShell's own parser instead.
        let script = powershell_setup_script("192.168.1.5", "NearbyPrint - Office");
        let file = job_dir().join("client-script-syntax-check.ps1");
        std::fs::write(&file, &script).expect("the generated script must be writable");

        let check = format!(
            "$t=$null;$e=$null;\
             [System.Management.Automation.Language.Parser]::ParseFile('{}',[ref]$t,[ref]$e)|Out-Null;\
             $e|ForEach-Object{{$_.Message}}",
            file.display()
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &check])
            .output();
        let _ = std::fs::remove_file(&file);

        match out {
            Ok(o) => {
                let report = String::from_utf8_lossy(&o.stdout).trim().to_string();
                assert!(
                    report.is_empty() && o.status.success(),
                    "generated script has a problem (status {:?})\nstdout: {report}\nstderr: {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            Err(e) => panic!("PowerShell is needed to verify the generated script: {e}"),
        }
    }

    fn local_script() -> String {
        LOCAL_PRINTER_SCRIPT
            .replace(
                "{{IPP_URL}}",
                &format!("http://{LOOPBACK}:{LOCAL_PROXY_PORT}/ipp/print"),
            )
            .replace("{{PROXY_PORT}}", &LOCAL_PROXY_PORT.to_string())
            .replace("{{PRINTER_NAME}}", VIRTUAL_PRINTER_NAME)
    }

    #[test]
    fn local_printer_script_points_at_this_machine_only() {
        let script = local_script();
        // Loopback on the dedicated proxy port, always, and always the IPP
        // path: the forward target lives in `relay-host` and is read when a
        // document arrives, so pointing the relay somewhere new must never
        // need a reinstall. The queue points at 8632 (loopback proxy), never
        // 8631 (the network listener phones connect to).
        assert!(script.contains("$Url       = 'http://127.0.0.1:8632/ipp/print'"));
        assert!(script.contains("$Port      = 8632"));
        assert!(script.contains("$Loopback  = '127.0.0.1'"));
        // The queue and the forward target are two different endpoints; the
        // relay route must never appear in anything Windows is asked to install.
        assert!(!script.contains("api/relay-print"));
        assert!(!script.contains("{{"), "unsubstituted placeholder in a shipped script");
        // Both install paths, the driver, and self-elevation - the same three
        // guarantees the network script makes.
        assert!(script.contains("-IppURL"));
        assert!(script.contains("'printui.dll,PrintUIEntry'"));
        assert!(script.contains("Microsoft IPP Class Driver"));
        assert!(script.contains("RunAs"));
        assert!(script.contains("Add-PrinterPort") == false);
        // The queue only works while the local server is up, and that has to
        // be said *before* Windows is asked to create anything.
        assert!(script.contains("BeginConnect($Loopback, $Port"));
        // Re-running repairs rather than fails: an existing queue pointing at
        // a different port is replaced, an identical one is left alone.
        assert!(script.contains("Remove-Printer -Name $Name"));
        assert!(script.contains("already installed"));
    }

    #[test]
    fn local_printer_script_parses_as_valid_powershell() {
        let script = local_script();
        let file = job_dir().join("virtual-printer-syntax-check.ps1");
        std::fs::write(&file, &script).expect("the generated script must be writable");

        let check = format!(
            "$t=$null;$e=$null;\
             [System.Management.Automation.Language.Parser]::ParseFile('{}',[ref]$t,[ref]$e)|Out-Null;\
             $e|ForEach-Object{{$_.Message}}",
            file.display()
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &check])
            .output();
        let _ = std::fs::remove_file(&file);

        match out {
            Ok(o) => {
                let report = String::from_utf8_lossy(&o.stdout).trim().to_string();
                assert!(
                    report.is_empty() && o.status.success(),
                    "generated script has a problem (status {:?})\nstdout: {report}\nstderr: {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            Err(e) => panic!("PowerShell is needed to verify the generated script: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // Two ports, two listeners
    // -----------------------------------------------------------------------

    #[test]
    fn the_proxy_and_the_network_server_never_share_a_port() {
        // The whole reason the proxy has its own port: both listeners run in
        // this one process on a client PC, and a second bind on a port the
        // first one already holds is `os error 10048`.
        assert_ne!(IPP_PORT, LOCAL_PROXY_PORT);
        assert_eq!(IPP_PORT, 8631);
        assert_eq!(LOCAL_PROXY_PORT, 8632);
        // The queue is handed the loopback endpoint; every other client is
        // handed the network one. Getting these the wrong way round is a
        // queue that points at nothing or a phone pointed at the proxy.
        assert_eq!(local_proxy_url(), "http://127.0.0.1:8632/ipp/print");
        let network = format!("http://192.168.1.5:{IPP_PORT}/ipp/print");
        assert_ne!(local_proxy_url(), network);
    }

    #[tokio::test]
    async fn a_second_listener_on_one_port_fails_the_way_we_now_avoid() {
        // Pins the actual failure this split exists to fix: two binds on the
        // same address are an error, never a second server. If someone ever
        // "simplifies" the two listeners back onto one port, this is the
        // error the Windows queue would start hitting again.
        let first = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("an ephemeral loopback port must be bindable");
        let taken = first.local_addr().expect("the bound address must be known");
        let second = tokio::net::TcpListener::bind(taken).await;
        let error = second.err().expect("the same port must refuse a second bind");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[test]
    fn proxy_uris_are_built_from_the_state_port() {
        // The proxy's copy of the state carries `LOCAL_PROXY_PORT`, and every
        // URI handed back to the spooler is built from that field - so a
        // printer-uri or job-uri can never lead a client of the loopback
        // proxy over to the network listener (or back).
        let mut proxy = test_state();
        proxy.local_ip = LOOPBACK.to_string();
        proxy.port = LOCAL_PROXY_PORT;
        assert_eq!(
            default_printer_uri(&proxy),
            "ipp://127.0.0.1:8632/ipp/print"
        );

        let mut network = test_state();
        network.local_ip = "192.168.1.5".into();
        network.port = IPP_PORT;
        assert_eq!(default_printer_uri(&network), "ipp://192.168.1.5:8631/ipp/print");
    }

    // -----------------------------------------------------------------------
    // mDNS discovery (client role)
    // -----------------------------------------------------------------------

    #[test]
    fn mdns_instance_name_is_the_part_the_user_recognises() {
        assert_eq!(
            mdns_instance_name("Office PC._ipp._tcp.local."),
            "Office PC"
        );
        // `ServiceRemoved` and `ServiceResolved` both hand over the full
        // name; the two must produce the same string or a host could be
        // added under one name and never removed.
        assert_eq!(
            mdns_instance_name("Office PC._ipp._tcp.local"),
            mdns_instance_name("Office PC._ipp._tcp.local.")
        );
        // A name with no service type at all passes through rather than
        // becoming an empty label.
        assert_eq!(mdns_instance_name("Office PC"), "Office PC");
    }

    #[test]
    fn discovered_host_from_describes_a_foreign_host() {
        // 203.0.113.7 is TEST-NET-3: a documentation range that will never
        // be one of *this* machine's addresses, so the self-filter below
        // cannot eat it by accident.
        let info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            "Office PC",
            "office-pc.local.",
            "203.0.113.7",
            IPP_PORT,
            None::<HashMap<String, String>>,
        )
        .expect("service info must build");

        let host = discovered_host_from(&info).expect("a foreign host must be offered");
        assert_eq!(host.name, "Office PC");
        assert_eq!(host.ip, "203.0.113.7");
        assert_eq!(host.port, IPP_PORT);
        // The default port is implied, so `set_relay_host` gets a bare
        // address - the same string the old manual field asked for.
        assert_eq!(host.host, "203.0.113.7");
        assert_eq!(host.url, "ipp://203.0.113.7:8631/ipp/print");
    }

    #[test]
    fn discovered_host_from_keeps_a_non_default_port_in_the_relay_target() {
        let info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            "Office PC",
            "office-pc.local.",
            "203.0.113.7",
            9000,
            None::<HashMap<String, String>>,
        )
        .expect("service info must build");

        let host = discovered_host_from(&info).expect("a foreign host must be offered");
        assert_eq!(host.port, 9000);
        // Without this, `set_relay_host` would default the port to 8631 and
        // every job would be forwarded to a listener that is not there.
        assert_eq!(host.host, "203.0.113.7:9000");
        assert_eq!(host.url, "ipp://203.0.113.7:9000/ipp/print");
    }

    #[test]
    fn discovered_host_from_never_offers_this_machine_to_itself() {
        // What our own advertisement looks like when it comes back to us:
        // every address in it belongs to this machine. Offering that in the
        // picker would make "choose a host" a choice between relaying to
        // ourselves and relaying to ourselves.
        let own = lan_ipv4s();
        assert!(
            !own.is_empty(),
            "this machine must have at least one LAN IPv4 for this test to mean anything"
        );
        let info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            "This PC",
            "this-pc.local.",
            own.join(","),
            IPP_PORT,
            None::<HashMap<String, String>>,
        )
        .expect("service info must build");
        assert_eq!(
            discovered_host_from(&info),
            None,
            "our own record must not appear as a host to print to"
        );

        // The same record carrying one foreign address still resolves - to
        // the foreign address, never to one of ours.
        let mixed = format!("{},203.0.113.7", own.join(","));
        let info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            "Office PC",
            "office-pc.local.",
            mixed,
            IPP_PORT,
            None::<HashMap<String, String>>,
        )
        .expect("service info must build");
        assert_eq!(
            discovered_host_from(&info).map(|host| host.ip),
            Some("203.0.113.7".to_string())
        );
    }

    #[test]
    fn relay_url_accepts_everything_the_field_can_reasonably_hold() {
        // A bare address means the app's own port, so pasting just an IP
        // cannot produce a URL that times out instead of failing to resolve.
        assert_eq!(
            relay_url("192.168.1.6").unwrap(),
            "http://192.168.1.6:8631/api/relay-print"
        );
        assert_eq!(
            relay_url(" 192.168.1.6:9000 ").unwrap(),
            "http://192.168.1.6:9000/api/relay-print"
        );
        assert_eq!(
            relay_url("http://192.168.1.6").unwrap(),
            "http://192.168.1.6:8631/api/relay-print"
        );
        // Whatever else is on the end of the address, the path is ours.
        assert_eq!(
            relay_url("http://192.168.1.6:9000/base/").unwrap(),
            "http://192.168.1.6:9000/api/relay-print"
        );
        // An empty field is "host mode", not a target.
        assert!(relay_url("").is_err());
        assert!(relay_url("   ").is_err());
        // No TLS stack is linked in, so saying so now beats a job failing
        // halfway through a print.
        assert!(relay_url("https://192.168.1.6").is_err());
        // Anything that is not host-shaped is rejected before it can be
        // persisted and turned into a silent forwarding failure later.
        assert!(relay_url("http://").is_err());
        assert!(relay_url("not a host!").is_err());
    }

    #[test]
    fn status_text_cannot_break_the_status_message_attribute() {
        assert_eq!(status_text("plain reason"), "plain reason");
        // Transport errors hand us arbitrary bytes; only 7-bit text may go on
        // the wire, and the reply must stay bounded.
        let long = "x".repeat(500);
        assert_eq!(status_text(&long).len(), 200);
        assert!(status_text("line\nbreak\tand\x00nul").bytes().all(|b| b >= 0x20));
    }

    #[tokio::test]
    async fn nothing_is_forwarded_until_a_host_is_configured() {
        let state = Arc::new(test_state());
        assert!(state.relay_host.lock().unwrap().is_empty());

        // The host role must be bit-for-bit the behaviour it was before: a
        // real payload on a Print-Job is still handled locally, not shipped
        // somewhere.
        let with_payload = request(
            Operation::PrintJob,
            40,
            &[(
                "document-format",
                IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
            )],
            SAMPLE_PDF,
        );
        assert!(relay_document(&state, &with_payload, peer()).await.is_none());
    }

    #[tokio::test]
    async fn only_a_document_operation_is_ever_forwarded() {
        let st = test_state();
        *st.relay_host.lock().unwrap() = "127.0.0.1:1".into();
        let state = Arc::new(st);

        // Discovery and job management are answered by this process even in
        // client role - a spooler that could not read the printer attributes
        // would never get as far as sending a document.
        let attrs_only = request(Operation::GetPrinterAttributes, 41, &[], &[]);
        assert!(relay_document(&state, &attrs_only, peer()).await.is_none());

        let jobs_only = request(Operation::GetJobs, 42, &[], &[]);
        assert!(relay_document(&state, &jobs_only, peer()).await.is_none());

        // Print-Job with no payload has nothing to forward; `build_response`
        // reports that it arrived empty.
        let no_payload = request(Operation::PrintJob, 43, &[], &[]);
        assert!(relay_document(&state, &no_payload, peer()).await.is_none());
    }

    #[tokio::test]
    async fn a_job_the_host_cannot_take_is_refused_rather_than_reported_printed() {
        let st = test_state();
        // Nothing listens on port 1, so this is an unreachable host.
        *st.relay_host.lock().unwrap() = "127.0.0.1:1".into();
        let state = Arc::new(st);

        let req = request(
            Operation::PrintJob,
            44,
            &[(
                "document-format",
                IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
            )],
            SAMPLE_PDF,
        );
        let bytes = relay_document(&state, &req, peer())
            .await
            .expect("a configured relay always answers")
            .expect("an unreachable host is an IPP error, not a transport crash");

        let (header, attrs) = parse(bytes);
        // `server-error-service-unavailable`, never SuccessfulOk: a spooler
        // that is told "printed" here loses the job with no record of it.
        assert_eq!(header.operation_or_status, 0x0508);
        assert_eq!(
            int_attr(&attrs, "job-state"),
            Some(RFC_JOB_STATE_ABORTED),
            "a job that never reached the printer is aborted, not completed"
        );

        let map = state.jobs.map.lock().unwrap();
        let job = map.values().next().expect("the failed job is still listed");
        assert_eq!(job.info.status, "error");
        // ...and it stays on disk so it can be printed from the list instead
        // of having to be sent a second time.
        let path = job.path.as_ref().expect("the document was kept");
        assert!(path.is_file());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn a_client_hands_the_document_over_and_the_host_spools_it() {
        // The host half, on an ephemeral port.
        let host_jobs = Arc::new(Jobs::default());
        let mut host_state = test_state();
        host_state.jobs = host_jobs.clone();
        let host_router = Router::new()
            .route(RELAY_PATH, post(relay_print))
            .with_state(Arc::new(host_state));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, host_router).await;
        });

        // The client half, pointed at it.
        let client_jobs = Arc::new(Jobs::default());
        let mut client_state = test_state();
        client_state.jobs = client_jobs.clone();
        *client_state.relay_host.lock().unwrap() = format!("127.0.0.1:{}", addr.port());
        let state = Arc::new(client_state);

        let req = request(
            Operation::PrintJob,
            45,
            &[(
                "document-format",
                IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
            )],
            SAMPLE_PDF,
        );
        let bytes = relay_document(&state, &req, peer())
            .await
            .expect("a configured relay always answers")
            .expect("the host is running, so the job must be accepted");

        let (header, attrs) = parse(bytes);
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(int_attr(&attrs, "job-state"), Some(RFC_JOB_STATE_COMPLETED));
        assert!(int_attr(&attrs, "job-id").is_some());
        assert!(req_str(&attrs, "job-uri").is_some());

        // The client records it as done and drops its own copy...
        {
            let map = client_jobs.map.lock().unwrap();
            let job = map.values().next().expect("the relayed job is listed");
            assert_eq!(job.info.status, "completed");
            assert!(job.path.is_none(), "a delivered document is not kept twice");
            assert!(job.info.file_path.is_none());
            assert!(job.info.device.starts_with("127.0.0.1:"));
        }

        // ...and the host has the very same bytes spooled. It announces the
        // job as `pending` rather than `printing` because a state with no
        // Tauri handle behind it cannot print anything - which is also the
        // branch a host with no printer selected takes, leaving the job for
        // the ordinary queue instead of silently dropping it.
        let map = host_jobs.map.lock().unwrap();
        let job = map.values().next().expect("the host received the job");
        assert_eq!(job.info.bytes, SAMPLE_PDF.len() as u64);
        assert_eq!(job.info.status, "pending");
        assert_eq!(job.info.device, "127.0.0.1");
        let path = job.path.as_ref().expect("the host spooled the document");
        assert_eq!(
            std::fs::read(path).expect("the spooled file is readable"),
            SAMPLE_PDF
        );
        let _ = std::fs::remove_file(path);
    }

    /// The job-template values the Mac's dialog sets travel all the way to
    /// the printer PC. The relay POST is plain document bytes, so copies and
    /// orientation only survive as headers - losing them there is how every
    /// relayed job used to come out as a single portrait copy.
    #[tokio::test]
    async fn relay_carries_copies_and_orientation_to_the_host() {
        let host_jobs = Arc::new(Jobs::default());
        let mut host_state = test_state();
        host_state.jobs = host_jobs.clone();
        let host_router = Router::new()
            .route(RELAY_PATH, post(relay_print))
            .with_state(Arc::new(host_state));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, host_router).await;
        });

        let client_state = test_state();
        *client_state.relay_host.lock().unwrap() = format!("127.0.0.1:{}", addr.port());
        let state = Arc::new(client_state);

        let req = request(
            Operation::PrintJob,
            460,
            &[
                (
                    "document-format",
                    IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
                ),
                ("copies", IppValue::Integer(3)),
                ("orientation-requested", IppValue::Enum(ORIENTATION_LANDSCAPE)),
            ],
            SAMPLE_PDF,
        );
        let bytes = relay_document(&state, &req, peer())
            .await
            .expect("a configured relay always answers")
            .expect("the host is running, so the job must be accepted");
        let (header, _) = parse(bytes);
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);

        let map = host_jobs.map.lock().unwrap();
        let job = map.values().next().expect("the host received the job");
        assert_eq!(
            job.options,
            PrintOptions {
                copies: 3,
                orientation: ORIENTATION_LANDSCAPE
            },
            "the host must print three landscape copies, not one portrait copy"
        );
        let path = job.path.as_ref().expect("the host spooled the document");
        let _ = std::fs::remove_file(path);
    }

    /// The wiring the tests above do not reach: `ipp_post` is the function the
    /// running server actually calls, and only it decides whether a request is
    /// forwarded or answered locally. A correct `relay_document` behind a
    /// wrong dispatch would never run, and nothing here would notice.
    #[tokio::test]
    async fn ipp_post_forwards_a_print_job_when_a_host_is_configured() {
        let host_jobs = Arc::new(Jobs::default());
        let mut host_state = test_state();
        host_state.jobs = host_jobs.clone();
        let host_router = Router::new()
            .route(RELAY_PATH, post(relay_print))
            .with_state(Arc::new(host_state));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, host_router).await;
        });

        let client_jobs = Arc::new(Jobs::default());
        let mut client_state = test_state();
        client_state.jobs = client_jobs.clone();
        *client_state.relay_host.lock().unwrap() = format!("127.0.0.1:{}", addr.port());
        let state = Arc::new(client_state);

        let req = request(
            Operation::PrintJob,
            46,
            &[(
                "document-format",
                IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
            )],
            SAMPLE_PDF,
        );
        let response = ipp_post(ConnectInfo(peer()), State(state), Bytes::from(req)).await;
        assert_eq!(response.status(), 200, "an IPP failure travels in the body");

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the reply body must be readable");
        let (header, attrs) = parse(bytes.to_vec());
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(int_attr(&attrs, "job-state"), Some(RFC_JOB_STATE_COMPLETED));

        // Recorded as delivered here and spooled over there: one job on each
        // side, and no second copy waiting to be printed twice.
        assert_eq!(client_jobs.map.lock().unwrap().len(), 1);
        assert_eq!(host_jobs.map.lock().unwrap().len(), 1);
    }

    /// The receiver must stop at our own queue rather than printing into it.
    /// This is the exact state this PC was found in: the persisted selection
    /// was the queue `Add to Windows` created, which points straight back at
    /// the server that would then relay it here again.
    #[tokio::test]
    async fn the_receiver_holds_a_job_instead_of_printing_into_our_own_queue() {
        let jobs = Arc::new(Jobs::default());
        let mut st = test_state();
        st.jobs = jobs.clone();
        // `test_state` calls its device "Test Printer", and an installed queue
        // is named after the device - so this is that queue.
        *st.selected_printer.lock().unwrap() = "Test Printer".to_string();
        let state = Arc::new(st);

        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/pdf".parse().unwrap());
        headers.insert("x-nearby-source", "192.168.1.99".parse().unwrap());

        let response = relay_print(State(state), headers, Bytes::from(SAMPLE_PDF.to_vec())).await;
        assert_eq!(response.status(), 200);

        let map = jobs.map.lock().unwrap();
        let job = map.values().next().expect("the job is not dropped");
        // Stopping the loop must not lose the document: it stays queued and
        // stays on disk, ready for the user to point it at a real printer.
        assert_eq!(job.info.status, "pending");
        assert!(job.path.is_some());
        assert!(
            job.info.message.contains("own queue"),
            "the reason has to be visible: {}",
            job.info.message
        );
        let _ = std::fs::remove_file(job.path.as_ref().unwrap());
    }

    #[test]
    fn only_queues_that_feed_back_into_us_are_recognized_as_ours() {
        // `Add to Windows` names the queue after the device...
        assert!(is_our_own_queue("NearbyPrint - Nootan", "NearbyPrint - Nootan"));
        assert!(is_our_own_queue("NEARBYPRINT - NOOTAN", "NearbyPrint - Nootan"));
        // ...the relay client's virtual printer is fixed...
        assert!(is_our_own_queue(VIRTUAL_PRINTER_NAME, "Something else"));
        // ...and a device renamed after the queue was installed still leaves
        // one pointing at us under the old, prefixed name.
        assert!(is_our_own_queue("NearbyPrint - Office", "Office Printer"));

        // Everything a real printer can be called passes through untouched.
        assert!(!is_our_own_queue("HP LaserJet (Shop)", "NearbyPrint - Nootan"));
        assert!(!is_our_own_queue("EPSON L3210 Series", "NearbyPrint - Nootan"));
        assert!(!is_our_own_queue("Microsoft Print to PDF", "NearbyPrint - Nootan"));
        assert!(!is_our_own_queue("", "NearbyPrint - Nootan"));
        assert!(!is_our_own_queue("   ", "NearbyPrint - Nootan"));
    }

    /// A job whose printer PC never answered is printed here instead - but only
    /// when there is a queue here that is not this app's own printer.
    #[tokio::test]
    async fn unreachable_relay_host_only_falls_back_to_a_queue_that_is_not_us() {
        let state = test_state();
        *state.relay_host.lock().unwrap() = "10.112.10.248".into();
        let (path, ..) = save_document_to_temp(SAMPLE_PDF, "application/pdf");
        let state = Arc::new(state);

        // Nothing chosen: there is nothing to fall back to, and printing on
        // "whatever Windows defaults to" is how a document ends up on the
        // wrong machine entirely.
        *state.selected_printer.lock().unwrap() = String::new();
        assert!(matches!(
            local_fallback(&state, &path, "t", PrintOptions::default()).await,
            LocalFallback::NoPrinter
        ));

        // Our own virtual printer must never be picked: it posts back into
        // this proxy, so the job would come round again and again.
        *state.selected_printer.lock().unwrap() = VIRTUAL_PRINTER_NAME.into();
        assert!(matches!(
            local_fallback(&state, &path, "t", PrintOptions::default()).await,
            LocalFallback::NoPrinter
        ));

        // Without a running app window there is nothing that could drive a
        // print, so it is still NoPrinter rather than a silent attempt.
        *state.selected_printer.lock().unwrap() = "HP LaserJet (Shop)".into();
        assert!(matches!(
            local_fallback(&state, &path, "t", PrintOptions::default()).await,
            LocalFallback::NoPrinter
        ));

        let _ = std::fs::remove_file(path);
    }

    /// The unreachable-host warning is set when a host cannot be reached and
    /// cleared again by one that takes a job - so it always names a host that
    /// is *currently* not answering.
    #[test]
    fn relay_warning_tracks_only_the_current_host() {
        let state = Arc::new(test_state());
        *state.relay_host.lock().unwrap() = "10.112.10.248".into();
        assert!(state.relay_warning.lock().unwrap().is_empty());

        notify_relay_fallback(&state, "10.112.10.248", "connection refused");
        let warning = state.relay_warning.lock().unwrap().clone();
        assert!(
            warning.contains("10.112.10.248"),
            "the warning must name the host, got {warning}"
        );
        assert!(
            warning.contains("connection refused"),
            "the warning must carry the reason, got {warning}"
        );

        clear_relay_warning(&state);
        assert!(
            state.relay_warning.lock().unwrap().is_empty(),
            "a job the host took proves it is back"
        );
    }

    /// Only a transport failure is retried locally: a host that *answered*
    /// has the document, and printing a second copy here would be worse than
    /// reporting the failure.
    #[test]
    fn only_unreachable_relay_failures_are_retried_locally() {
        match RelayFailure::Unreachable("no route".into()) {
            RelayFailure::Unreachable(reason) => assert_eq!(reason, "no route"),
            RelayFailure::Refused(reason) => panic!("wrong variant: {reason}"),
        }
        assert_eq!(
            RelayFailure::Refused("busy".into()).reason(),
            "busy",
            "the reason must survive for the spooler either way"
        );
        assert_eq!(
            RelayFailure::Unreachable("gone".into()).reason(),
            "gone",
            "the reason must survive for the spooler either way"
        );
    }

    #[test]
    fn spooled_documents_never_get_the_bin_extension() {
        // The silent printer rejects anything that is not `.pdf`.
        assert_eq!(extension_for("", b"%PDF-1.7\n%%EOF"), "pdf");
        assert_eq!(extension_for("", &[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(extension_for("", b"RaS2xxxx"), "pwg");
        assert_eq!(extension_for("", b"UNIRAST"), "urf");

        // What macOS queues installed with a PostScript driver send instead.
        assert_eq!(extension_for("", b"%!PS-Adobe-3.0\n"), "ps");
        assert_eq!(extension_for("application/postscript", b"whatever"), "ps");

        // Anything else falls through to `.pdf` - never the old `.bin`.
        assert_eq!(extension_for("", &[0x13, 0x37]), "pdf");
        assert_eq!(extension_for("", &[]), "pdf");

        let (path, filename, ext, _) = save_document_to_temp(b"%!PS-Adobe-3.0\nshowpage\n", "");
        assert_eq!(ext, "ps");
        assert!(filename.ends_with(".ps"), "got {filename}");
        let _ = std::fs::remove_file(path);

        let (path, filename, ext, _) = save_document_to_temp(&[0x13, 0x37], "application/pdf");
        assert_eq!(ext, "pdf");
        assert!(filename.ends_with(".pdf"), "got {filename}");
        let _ = std::fs::remove_file(path);

        // PDFs are handed to the printer untouched.
        assert!(prepare_document("application/pdf", b"%PDF-1.4\n")
            .unwrap()
            .is_none());
        assert!(is_postscript("", b"%!PS-Adobe-3.0"));
        assert!(is_postscript("application/postscript", b"anything"));
        assert!(!is_postscript("application/pdf", b"%PDF-1.4"));
    }

    /// The whole macOS fix in one assertion: PostScript in, real PDF out.
    #[test]
    #[cfg(windows)]
    fn postscript_jobs_are_converted_to_pdf() {
        let gs = match find_ghostscript() {
            Some(gs) => gs,
            None => {
                println!("ghostscript not installed - skipping conversion check");
                return;
            }
        };
        println!("converting with {}", gs.display());

        let ps = b"%!PS-Adobe-3.0\n/Helvetica findfont 24 scalefont setfont\n\
                   72 720 moveto (Nearby Print) show\nshowpage\n%%EOF\n";
        let pdf = postscript_to_pdf(ps).expect("ghostscript should render PostScript");
        assert!(pdf.starts_with(b"%PDF"), "converted document must be a real PDF");
    }

    /// End-to-end check of feature 2 against this machine: hidden PowerShell
    /// round-trip + JSON parsing + per-printer online decision.
    #[test]
    #[cfg(windows)]
    fn live_printer_status_query_finds_every_queue() {
        let status = platform_printer_statuses();
        assert!(
            !status.is_empty(),
            "the Win32_Printer query should list this PC's queues"
        );
        for (name, online) in &status {
            println!("  {name}: {}", if *online { "online" } else { "offline" });
        }
    }

    #[test]
    fn get_jobs_returns_empty() {
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::GetJobs, 4, &[], &[]));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert!(int_attr(&attrs, "job-id").is_none());
        assert_eq!(int_attr(&attrs, "number-of-jobs"), Some(0));
    }

    #[test]
    fn get_jobs_lists_the_job_the_client_is_waiting_on() {
        // A client that has just handed over a document asks Get-Jobs where it
        // went. A reply with no job groups is not read as "the printer is
        // idle" by that client - it cannot find its own job, keeps asking, and
        // once it has asked long enough submits the whole file again. That is
        // how one print from a phone showed up in this server's log as five
        // identical Print-Jobs.
        let state = test_state();
        let (_, accepted) = exchange(&state, request(Operation::PrintJob, 40, &[], SAMPLE_PDF));
        let job_id = int_attr(&accepted, "job-id").expect("Print-Job must hand out a job-id");

        let (header, attrs) = exchange(&state, request(Operation::GetJobs, 41, &[], &[]));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert_eq!(int_attr(&attrs, "number-of-jobs"), Some(1));
        assert_eq!(int_attr(&attrs, "job-id"), Some(job_id));

        let job_state = int_attr(&attrs, "job-state").expect("job-state is mandatory");
        assert!(
            (3..=9).contains(&job_state),
            "job-state must be an RFC 8011 4.3.7 value, got {job_state}"
        );
        assert!(
            req_str(&attrs, "job-name").is_some(),
            "job-name is what lets the client pick its own job out of the list"
        );
        assert!(req_str(&attrs, "job-uri").is_some(), "job-uri must be present");
        assert!(
            !is_terminal_job_state(job_state),
            "a job nobody printed yet must not read as finished"
        );

        if let Some(p) = state.jobs.take_path(job_id as u32) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn get_job_attributes_agrees_with_get_jobs() {
        // Both operations describe the same job. Answering "finished" from one
        // and "still running" from the other leaves the client trusting neither
        // answer, and it goes back to polling indefinitely.
        let state = test_state();
        let (_, accepted) = exchange(&state, request(Operation::PrintJob, 42, &[], SAMPLE_PDF));
        let job_id = int_attr(&accepted, "job-id").expect("Print-Job must hand out a job-id");

        let (_, attrs) = exchange(
            &state,
            request(
                Operation::GetJobAttributes,
                43,
                &[("job-id", IppValue::Integer(job_id))],
                &[],
            ),
        );
        assert_eq!(int_attr(&attrs, "job-id"), Some(job_id));
        // 5 = processing: the bytes are in the pipeline. What must *not*
        // appear is 9, which would tell the client the job is finished while
        // it has not reached the engine yet.
        assert_eq!(
            int_attr(&attrs, "job-state"),
            Some(5),
            "a job that has been received but not finished is processing, not completed"
        );
        assert!(req_str(&attrs, "job-name").is_some());

        if let Some(p) = state.jobs.take_path(job_id as u32) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn internal_job_states_never_reach_the_wire_unmapped() {
        // `Job::ipp_state` and the RFC 8011 4.3.7 enumeration are numbered
        // differently on purpose; sending one as the other hands the client a
        // value its spec has no meaning for, which it treats as "unknown" and
        // keeps asking about.
        assert_eq!(rfc_job_state(JOB_PENDING), 3);
        assert_eq!(rfc_job_state(JOB_PROCESSING), 5);
        assert_eq!(rfc_job_state(JOB_CANCELED), 7);
        assert_eq!(rfc_job_state(JOB_ABORTED), 8);
        assert_eq!(rfc_job_state(JOB_COMPLETED), 9);

        assert!(!is_terminal_job_state(rfc_job_state(JOB_PENDING)));
        assert!(!is_terminal_job_state(rfc_job_state(JOB_PROCESSING)));
        assert!(is_terminal_job_state(rfc_job_state(JOB_CANCELED)));
        assert!(is_terminal_job_state(rfc_job_state(JOB_ABORTED)));
        assert!(is_terminal_job_state(rfc_job_state(JOB_COMPLETED)));
    }

    #[test]
    fn get_printer_name_from_state() {
        let state = test_state();
        assert_eq!(state.printer_name, "Test Printer");
    }

    #[test]
    fn validate_job_is_acknowledged_with_successful_ok() {
        let state = test_state();
        let (header, attrs) = exchange(&state, request(Operation::ValidateJob, 5, &[], &[]));
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
        assert!(req_str(&attrs, "attributes-charset").is_some());
        assert!(req_str(&attrs, "attributes-natural-language").is_some());
        assert!(req_str(&attrs, "printer-name").is_none());
    }

    #[test]
    fn validate_job_with_document_still_returns_successful_ok() {
        let state = test_state();
        let (header, _) = exchange(
            &state,
            request(
                Operation::ValidateJob,
                6,
                &[(
                    "document-format",
                    IppValue::MimeMediaType("application/pdf".try_into().unwrap()),
                )],
                SAMPLE_PDF,
            ),
        );
        assert_eq!(header.operation_or_status, StatusCode::SuccessfulOk as u16);
    }

    #[test]
    fn mdns_txt_records_declare_a_printer() {
        // A spooler (Mopria / Windows) ignores an `_ipp._tcp` answer whose TXT
        // record does not describe a printer, so assert the exact set that is
        // attached to the ServiceInfo before `register()`.
        let info = ServiceInfo::new(
            "_ipp._tcp.local.",
            "Test Printer",
            "NearbyPrint.local.",
            "192.168.1.5",
            IPP_PORT,
            Some(txt_property_map()),
        )
        .expect("service info must build");

        assert_eq!(info.get_fullname(), "Test Printer._ipp._tcp.local.");
        assert_eq!(info.get_port(), IPP_PORT);
        assert_eq!(
            info.get_properties().len(),
            9,
            "exactly the nine TXT keys AirPrint discovery reads"
        );

        for (key, value) in txt_properties() {
            assert_eq!(
                info.get_property_val_str(key),
                Some(value),
                "TXT record must carry {key}={value}"
            );
        }

        // The subset a Mac/phone actually filters on when it decides whether
        // this is a printer worth listing. Dropping any one of them is what
        // made the printer invisible to AirPrint after an IP change.
        for key in [
            "txtvers", "qtotal", "rp", "ty", "note", "pdl", "URF", "Color", "Duplex",
        ] {
            assert!(
                info.get_property_val_str(key).is_some(),
                "AirPrint clients read {key}"
            );
        }
        assert_eq!(info.get_property_val_str("rp"), Some("ipp/print"));
        assert_eq!(info.get_property_val_str("pdl"), Some("application/pdf,image/urf"));
    }

    /// A service registered under the `_universal` AirPrint subtype still
    /// answers as a plain `_ipp._tcp` instance: iOS only ever queries the
    /// subtype form, while Windows and older tools query the parent, and both
    /// have to land on this same server.
    #[test]
    fn airprint_subtype_registers_under_the_parent_ipp_service() {
        let info = ServiceInfo::new(
            AIRPRINT_SERVICE_TYPE,
            "Test Printer",
            "NearbyPrint.local.",
            "192.168.1.5",
            IPP_PORT,
            Some(txt_property_map()),
        )
        .expect("subtype service info must build");

        assert_eq!(
            info.get_fullname(),
            "Test Printer._ipp._tcp.local.",
            "the instance must stay under the parent service so an ordinary _ipp._tcp query still finds it"
        );
        assert_eq!(info.get_port(), IPP_PORT);
        assert_eq!(info.get_property_val_str("rp"), Some("ipp/print"));
    }

    /// Copies and orientation are parsed out of the job-template group and
    /// survive the merge with the options a Create-Job already stored - the
    /// two-step Mac flow that sends them once and never repeats them.
    #[test]
    fn print_options_read_the_job_template_group() {
        /// The job-template group the client puts these in (RFC 8011 4.2).
        fn job_template(values: &[(&str, IppValue)]) -> IppAttributes {
            let mut attrs = IppAttributes::new();
            for (name, value) in values {
                attrs.add(
                    DelimiterTag::JobAttributes,
                    IppAttribute::new((*name).try_into().unwrap(), value.clone()),
                );
            }
            attrs
        }

        let attrs = job_template(&[]);
        assert_eq!(
            PrintOptions::from_request(&attrs, PrintOptions::default()),
            PrintOptions::default(),
            "a silent request is one portrait copy"
        );

        let attrs = job_template(&[
            ("copies", IppValue::Integer(3)),
            ("orientation-requested", IppValue::Enum(ORIENTATION_LANDSCAPE)),
        ]);
        assert_eq!(
            PrintOptions::from_request(&attrs, PrintOptions::default()),
            PrintOptions {
                copies: 3,
                orientation: ORIENTATION_LANDSCAPE
            }
        );

        // Out-of-range values fall back rather than reaching the engine.
        let attrs = job_template(&[
            ("copies", IppValue::Integer(0)),
            ("orientation-requested", IppValue::Enum(42)),
        ]);
        let parsed = PrintOptions::from_request(&attrs, PrintOptions::default());
        assert_eq!(parsed.copies, 1, "zero copies must clamp to one");
        assert_eq!(
            parsed.orientation,
            ORIENTATION_PORTRAIT,
            "an orientation outside 3..=6 is the portrait default, not a value the engine understands"
        );

        // Send-Document that repeats nothing keeps what Create-Job stored:
        // that is the two-step Mac flow, and losing it here is what printed a
        // single portrait copy no matter what the dialog said.
        let carried = PrintOptions {
            copies: 4,
            orientation: ORIENTATION_REVERSE_PORTRAIT,
        };
        assert_eq!(
            PrintOptions::from_request(&job_template(&[]), carried),
            carried
        );

        // A value the request *does* repeat overrides the stored one.
        let repeated = job_template(&[("copies", IppValue::Integer(2))]);
        assert_eq!(
            PrintOptions::from_request(&repeated, carried),
            PrintOptions {
                copies: 2,
                orientation: ORIENTATION_REVERSE_PORTRAIT
            }
        );
    }

    /// The exact `-print-settings` list handed to SumatraPDF.
    #[test]
    fn sumatra_settings_carry_copies_and_orientation() {
        assert_eq!(
            PrintOptions::default().sumatra_print_settings(),
            "",
            "one portrait copy needs no settings at all - the plain command stays untouched"
        );
        assert_eq!(
            PrintOptions {
                copies: 3,
                orientation: ORIENTATION_PORTRAIT
            }
            .sumatra_print_settings(),
            "3x"
        );
        assert_eq!(
            PrintOptions {
                copies: 1,
                orientation: ORIENTATION_LANDSCAPE
            }
            .sumatra_print_settings(),
            "landscape"
        );
        assert_eq!(
            PrintOptions {
                copies: 2,
                orientation: ORIENTATION_LANDSCAPE
            }
            .sumatra_print_settings(),
            "2x,landscape"
        );
        assert_eq!(
            PrintOptions {
                copies: 1,
                orientation: ORIENTATION_REVERSE_LANDSCAPE
            }
            .sumatra_print_settings(),
            "landscape,rotate=180"
        );
        assert_eq!(
            PrintOptions {
                copies: 1,
                orientation: ORIENTATION_REVERSE_PORTRAIT
            }
            .sumatra_print_settings(),
            "portrait,rotate=180"
        );
    }

    /// `-print-settings` only appears when the job actually asks for
    /// something other than one portrait copy.
    #[test]
    fn sidecar_adds_print_settings_only_when_they_are_needed() {
        let plain = sidecar_args(
            "HP LaserJet",
            Path::new(r"C:\jobs\doc.pdf"),
            PrintOptions::default(),
        );
        assert!(
            !plain.iter().any(|a| a == "-print-settings"),
            "the single-copy portrait command must stay byte-for-byte the old one"
        );

        let fancy = sidecar_args(
            "HP LaserJet",
            Path::new(r"C:\jobs\doc.pdf"),
            PrintOptions {
                copies: 3,
                orientation: ORIENTATION_LANDSCAPE,
            },
        );
        let pos = fancy.iter().position(|a| a == "-print-settings").expect(
            "-print-settings is how the copies and the orientation reach SumatraPDF",
        );
        assert_eq!(fancy[pos + 1], "3x,landscape");
        // Still last before the file: SumatraPDF reads every flag up to it.
        assert_eq!(fancy[pos + 2], "-exit-when-done");
    }
}
