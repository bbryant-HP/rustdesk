//! Heritage Help Desk: device stats for the technician's Device info panel.
//!
//! The service already checks in with the API server (src/hbbs_http/sync.rs): a heartbeat
//! every 15 seconds, every 3 while someone is connected, and a sysinfo upload whenever that
//! changes. This adds live usage to the heartbeat (`live`) and hardware and network details
//! to the sysinfo upload (`add_inventory`). Nothing here talks to the network itself.
//!
//! Kept in its own file so merging upstream RustDesk releases only touches a few lines.

use hbb_common::{
    log,
    sysinfo::{Disks, System},
};
use serde_json::{json, Value};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// How long hardware details are reused before being read again.
const HARDWARE_MAX_AGE: Duration = Duration::from_secs(6 * 3600);
/// How long a failed hardware read waits before it is tried again.
const HARDWARE_RETRY: Duration = Duration::from_secs(600);
/// How long network details are reused; adapters rarely change, and reading them is not free.
const NETWORK_MAX_AGE: Duration = Duration::from_secs(30);

lazy_static::lazy_static! {
    // Kept between calls: CPU usage is measured as the change since the previous refresh.
    static ref SYSTEM: Mutex<System> = Mutex::new(System::new());
    static ref HARDWARE: Mutex<Hardware> = Mutex::new(Hardware::default());
    static ref NETWORK: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
}

#[derive(Default)]
struct Hardware {
    value: Option<Value>,
    read_at: Option<Instant>,
    reading: bool,
}

/// Live usage, sent with every heartbeat.
pub fn live() -> Value {
    let mut sys = SYSTEM.lock().unwrap();
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let cpu = (sys.global_cpu_info().cpu_usage() as f64 * 10.).round() / 10.;
    let mut disks: Vec<Value> = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| {
            json!({
                "mount": d.mount_point().to_string_lossy(),
                "label": d.name().to_string_lossy(),
                "fs": d.file_system().to_string_lossy(),
                "kind": format!("{:?}", d.kind()),
                "removable": d.is_removable(),
                "total": d.total_space(),
                "free": d.available_space(),
            })
        })
        .collect();
    disks.sort_by(|a, b| a["mount"].as_str().cmp(&b["mount"].as_str()));
    json!({
        "cpu": cpu,
        "mem_total": sys.total_memory(),
        "mem_used": sys.used_memory(),
        "disks": disks,
        "uptime": sys.uptime(),
    })
}

/// Adds hardware and network details to the sysinfo upload. Both change rarely, and the client
/// only re-uploads sysinfo when its content changes, so this costs nothing most of the time.
pub fn add_inventory(v: &mut Value) {
    let hw = hardware();
    if !hw.is_null() {
        v["hw"] = hw;
    }
    v["net"] = network(&v["hw"]);
}

fn network(hw: &Value) -> Value {
    let mut cache = NETWORK.lock().unwrap();
    if let Some((at, value)) = cache.as_ref() {
        if at.elapsed() < NETWORK_MAX_AGE {
            return with_dns(value.clone(), hw);
        }
    }
    let mut list: Vec<Value> = default_net::get_interfaces()
        .into_iter()
        .filter(|i| format!("{:?}", i.if_type) != "Loopback")
        .filter(|i| i.mac_addr.is_some() && !i.ipv4.is_empty())
        .map(|i| {
            let speed = i
                .receive_speed
                .or(i.transmit_speed)
                .map(|s| s / 1_000_000)
                .unwrap_or(0);
            json!({
                "index": i.index,
                "name": i.friendly_name.clone().unwrap_or_else(|| i.name.clone()),
                "description": i.description.clone().unwrap_or_default(),
                "type": format!("{:?}", i.if_type),
                "mac": i.mac_addr.map(|m| m.address().to_uppercase()).unwrap_or_default(),
                "ipv4": i.ipv4.iter().map(|n| format!("{}/{}", n.addr, n.prefix_len)).collect::<Vec<_>>(),
                "ipv6": i.ipv6.iter().map(|n| n.addr.to_string()).collect::<Vec<_>>(),
                "gateway": i.gateway.as_ref().map(|g| g.ip_addr.to_string()).unwrap_or_default(),
                "speed_mbps": speed,
            })
        })
        .collect();
    // Adapters with a gateway first: that's the one carrying the traffic.
    list.sort_by_key(|n| (n["gateway"].as_str().unwrap_or("").is_empty(), n["name"].as_str().unwrap_or("").to_owned()));
    let value = Value::Array(list);
    *cache = Some((Instant::now(), value.clone()));
    with_dns(value, hw)
}

/// DNS servers come from the hardware read (Windows only), matched by adapter index.
fn with_dns(mut net: Value, hw: &Value) -> Value {
    if let (Some(list), Some(dns)) = (net.as_array_mut(), hw["dns"].as_array()) {
        for n in list.iter_mut() {
            let servers: Vec<Value> = dns
                .iter()
                .filter(|d| d["index"].as_u64() == n["index"].as_u64())
                .flat_map(|d| d["servers"].as_array().cloned().unwrap_or_default())
                .collect();
            if !servers.is_empty() {
                n["dns"] = Value::Array(servers);
            }
        }
    }
    net
}

/// Hardware details, read in the background so a slow read never holds up the heartbeat.
/// Returns Null until the first read finishes.
fn hardware() -> Value {
    let mut hw = HARDWARE.lock().unwrap();
    let max_age = if hw.value.is_some() { HARDWARE_MAX_AGE } else { HARDWARE_RETRY };
    let stale = hw.read_at.map(|t| t.elapsed() >= max_age).unwrap_or(true);
    if stale && !hw.reading {
        hw.reading = true;
        std::thread::spawn(|| {
            let value = read_hardware();
            let mut hw = HARDWARE.lock().unwrap();
            if value.is_some() {
                hw.value = value;
            }
            hw.read_at = Some(Instant::now());
            hw.reading = false;
        });
    }
    hw.value.clone().unwrap_or(Value::Null)
}

#[cfg(windows)]
const HARDWARE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
function D($d) { if ($d) { $d.ToString('yyyy-MM-dd HH:mm') } else { '' } }
$cs = Get-CimInstance Win32_ComputerSystem
$b = Get-CimInstance Win32_BIOS
$os = Get-CimInstance Win32_OperatingSystem
$p = @(Get-CimInstance Win32_Processor)[0]
$bb = Get-CimInstance Win32_BaseBoard
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$disks = @(Get-PhysicalDisk | Sort-Object DeviceId | ForEach-Object {
  [ordered]@{ model = "$($_.FriendlyName)".Trim(); media = "$($_.MediaType)"; bus = "$($_.BusType)";
              size = [int64]$_.Size; health = "$($_.HealthStatus)"; serial = "$($_.SerialNumber)".Trim() } })
$dns = @(Get-DnsClientServerAddress -AddressFamily IPv4 | Where-Object { $_.ServerAddresses } | ForEach-Object {
  [ordered]@{ index = $_.InterfaceIndex; servers = @($_.ServerAddresses) } })
[ordered]@{
  manufacturer = "$($cs.Manufacturer)".Trim(); model = "$($cs.Model)".Trim(); serial = "$($b.SerialNumber)".Trim()
  board = "$($bb.Manufacturer) $($bb.Product)".Trim(); bios = "$($b.SMBIOSBIOSVersion)".Trim(); bios_date = D $b.ReleaseDate
  cpu = "$($p.Name)".Trim(); cores = $p.NumberOfCores; threads = $p.NumberOfLogicalProcessors
  ram = [int64]$cs.TotalPhysicalMemory
  domain = "$($cs.Domain)"; joined = [bool]$cs.PartOfDomain
  os = "$($os.Caption)".Trim(); os_display = "$($cv.DisplayVersion)"; os_build = "$($os.Version).$($cv.UBR)"
  os_arch = "$($os.OSArchitecture)"; installed = D $os.InstallDate; last_boot = D $os.LastBootUpTime
  disks = $disks; dns = $dns
} | ConvertTo-Json -Compress -Depth 4
"#;

#[cfg(windows)]
fn read_hardware() -> Option<Value> {
    use std::{io::Read, os::windows::process::CommandExt, process::{Command, Stdio}};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // -EncodedCommand takes UTF-16LE base64, which sidesteps every quoting problem.
    let utf16: Vec<u8> = HARDWARE_SCRIPT.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
    let exe = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut child = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(hbb_common::base64::encode(&utf16))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| log::warn!("Device info: can't start PowerShell: {e}"))
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok();
        out
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(60) => std::thread::sleep(Duration::from_millis(250)),
            _ => {
                log::warn!("Device info: hardware read timed out");
                child.kill().ok();
                child.wait().ok();
                return None;
            }
        }
    }
    let out = reader.join().ok()?;
    match serde_json::from_str::<Value>(out.trim()) {
        Ok(v) if v.is_object() => Some(v),
        _ => {
            log::warn!("Device info: unexpected hardware read output ({} bytes)", out.len());
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn read_hardware() -> Option<Value> {
    // The service runs as root, so even the serial number is readable.
    let dmi = |f: &str| {
        std::fs::read_to_string(format!("/sys/class/dmi/id/{f}"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    Some(json!({
        "manufacturer": dmi("sys_vendor"),
        "model": dmi("product_name"),
        "serial": dmi("product_serial"),
        "board": format!("{} {}", dmi("board_vendor"), dmi("board_name")).trim().to_owned(),
        "bios": dmi("bios_version"),
        "bios_date": dmi("bios_date"),
    }))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn read_hardware() -> Option<Value> {
    None
}
