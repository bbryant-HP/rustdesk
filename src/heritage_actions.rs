//! Heritage Help Desk: installed programs and services, and admins' uninstall and service commands.
//!
//! The server asks for the programs and services lists (`"software": true` in the `heritage` part
//! of a heartbeat response) while someone has those tabs open, and passes on admins' commands as
//! `"actions"`. A command only runs if it is signed with the key this install was built with (the
//! same one that signs its custom.txt), names this device, and hasn't expired. The server never
//! sends a command line: an uninstall names a registry entry, and only the silent uninstaller
//! that entry lists on this device runs.
//!
//! Kept in its own file so merging upstream RustDesk releases only touches a few lines.

use hbb_common::{config::Config, log};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Mutex, time::Duration};

const ACTION_PREFIX: &[u8] = b"heritage-action:";

lazy_static::lazy_static! {
    static ref SCREEN_BUSY: Mutex<bool> = Mutex::new(false);
    static ref SOFTWARE_BUSY: Mutex<bool> = Mutex::new(false);
    // Commands already started, by id, with their result once finished (re-sent if asked again).
    static ref ACTIONS: Mutex<HashMap<String, Option<Value>>> = Default::default();
}

/// The `heritage` part of a heartbeat response.
pub fn handle(h: Value, heartbeat_url: &str) {
    let base = heartbeat_url.trim_end_matches("/heartbeat").to_owned(); // ".../api"
    if h["software"].as_bool() == Some(true) {
        refresh_software(base.clone());
    }
    if h["screen"].as_bool() == Some(true) {
        send_screen(base.clone());
    }
    if let Some(list) = h["actions"].as_array() {
        for a in list {
            if let Some(s) = a.as_str() {
                start_action(s, base.clone());
            }
        }
    }
}

fn refresh_software(base: String) {
    {
        let mut busy = SOFTWARE_BUSY.lock().unwrap();
        if *busy {
            return;
        }
        *busy = true;
    }
    std::thread::spawn(move || {
        let lists = read_software();
        *SOFTWARE_BUSY.lock().unwrap() = false;
        let mut body = lists.unwrap_or_else(
            || json!({"error": "Couldn't read the programs and services on this device."}),
        );
        post(&base, "software", &mut body);
    });
}

/// A small picture of the main screen, like ScreenConnect's thumbnails. The server asks for one
/// only while someone is looking at this device in a list or in Device info.
fn send_screen(base: String) {
    {
        let mut busy = SCREEN_BUSY.lock().unwrap();
        if *busy {
            return;
        }
        *busy = true;
    }
    std::thread::spawn(move || {
        let shot = read_screen();
        *SCREEN_BUSY.lock().unwrap() = false;
        let mut body =
            shot.unwrap_or_else(|| json!({"error": "The screen couldn't be captured."}));
        post(&base, "screen", &mut body);
    });
}

const NOTHING_TO_SHOW: &str =
    "Nothing to show: the PC is at the sign-in or lock screen, or no one is signed in.";

/// Copies the main screen with plain Windows GDI calls (no script: antivirus rightly blocks
/// PowerShell that captures the screen), then shrinks it to a JPEG at most 960 pixels wide. This
/// runs in the help desk's process in the signed-in user's session, which is per-monitor DPI
/// aware, so the copy is full resolution.
#[cfg(windows)]
fn read_screen() -> Option<Value> {
    let (w, h, bgra) = match capture_main_screen() {
        Ok(shot) => shot,
        Err(e) => return Some(json!({ "error": e })),
    };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let img = image::RgbImage::from_raw(w as u32, h as u32, rgb)?;
    let (tw, th) = if w > 960 {
        (960u32, ((h * 960) / w).max(1) as u32)
    } else {
        (w as u32, h as u32)
    };
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60)
        .encode(small.as_raw(), tw, th, image::ColorType::Rgb8)
        .ok()?;
    Some(json!({
        "jpeg": hbb_common::base64::encode(&jpeg),
        "width": tw,
        "height": th,
    }))
}

#[cfg(windows)]
fn capture_main_screen() -> Result<(usize, usize, Vec<u8>), &'static str> {
    use std::ptr::null_mut;
    use winapi::um::{
        wingdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
            GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
            DIB_RGB_COLORS, SRCCOPY,
        },
        winuser::{GetDC, GetSystemMetrics, ReleaseDC, SM_CXSCREEN, SM_CYSCREEN},
    };
    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w <= 0 || h <= 0 {
            return Err(NOTHING_TO_SHOW);
        }
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return Err(NOTHING_TO_SHOW);
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut lines = 0;
        if !mem.is_null() && !bmp.is_null() {
            let old = SelectObject(mem, bmp as _);
            let copied = BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY | CAPTUREBLT) != 0;
            SelectObject(mem, old);
            if copied {
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = w;
                info.bmiHeader.biHeight = -h; // top row first
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB;
                lines = GetDIBits(
                    mem,
                    bmp,
                    0,
                    h as u32,
                    buf.as_mut_ptr() as _,
                    &mut info,
                    DIB_RGB_COLORS,
                );
            }
        }
        if !bmp.is_null() {
            DeleteObject(bmp as _);
        }
        if !mem.is_null() {
            DeleteDC(mem);
        }
        ReleaseDC(null_mut(), screen);
        // An all-black copy means the desktop isn't ours to see (sign-in or lock screen).
        if lines == 0 || buf.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0) {
            return Err(NOTHING_TO_SHOW);
        }
        Ok((w as usize, h as usize, buf))
    }
}

#[cfg(not(windows))]
fn read_screen() -> Option<Value> {
    None
}

fn post(base: &str, path: &str, body: &mut Value) {
    body["id"] = json!(Config::get_id());
    body["uuid"] = json!(crate::encode64(hbb_common::get_uuid()));
    let url = format!("{base}/heritage/{path}");
    match hbb_common::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            if let Err(e) = rt.block_on(crate::post_request(url, body.to_string(), "")) {
                log::warn!("Device info: sending {path} failed: {e:?}");
            }
        }
        Err(e) => log::warn!("Device info: no runtime to send {path}: {e:?}"),
    }
}

fn start_action(signed: &str, base: String) {
    let cmd = match verify_action(signed) {
        Ok(cmd) => cmd,
        Err(e) => {
            log::warn!("Device info: refused a command: {e}");
            return;
        }
    };
    let id = cmd["action"].as_str().unwrap_or_default().to_owned();
    {
        let mut started = ACTIONS.lock().unwrap();
        match started.get(&id) {
            Some(Some(result)) => {
                // Already done, but the server didn't get the result: send it again.
                let mut result = result.clone();
                std::thread::spawn(move || post(&base, "action-result", &mut result));
                return;
            }
            Some(None) => return, // still running
            None => {
                started.insert(id.clone(), None);
            }
        }
    }
    std::thread::spawn(move || {
        log::info!(
            "Device info: running {} {} {}",
            cmd["kind"],
            cmd["op"],
            cmd["target"]
        );
        let (ok, message) = run_action(&cmd);
        let mut result = json!({"action": id, "ok": ok, "message": message});
        ACTIONS.lock().unwrap().insert(id, Some(result.clone()));
        post(&base, "action-result", &mut result);
    });
}

fn verify_action(signed: &str) -> Result<Value, &'static str> {
    use hbb_common::sodiumoxide::crypto::sign;
    let key = hbb_common::config::HARD_SETTINGS
        .read()
        .unwrap()
        .get("heritage-action-key")
        .cloned()
        .unwrap_or_default();
    if key.is_empty() {
        return Err("this install has no command key; install the current download");
    }
    let pk = hbb_common::base64::decode(&key)
        .ok()
        .and_then(|k| sign::PublicKey::from_slice(&k))
        .ok_or("the command key is unreadable")?;
    let data = hbb_common::base64::decode(signed).map_err(|_| "not base64")?;
    let msg = sign::verify(&data, &pk).map_err(|_| "bad signature")?;
    let body = msg
        .as_slice()
        .strip_prefix(ACTION_PREFIX)
        .ok_or("not a command")?;
    let cmd: Value = serde_json::from_slice(body).map_err(|_| "unreadable command")?;
    if cmd["device"].as_str() != Some(Config::get_id().as_str()) {
        return Err("meant for another device");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let exp = cmd["exp"].as_i64().unwrap_or(0);
    if exp < now || exp > now + 3600 {
        return Err("expired");
    }
    if cmd["action"].as_str().unwrap_or_default().is_empty() {
        return Err("no command id");
    }
    Ok(cmd)
}

#[cfg(windows)]
fn read_software() -> Option<Value> {
    crate::heritage_stats::run_powershell(SOFTWARE_SCRIPT, Duration::from_secs(120))
}

#[cfg(not(windows))]
fn read_software() -> Option<Value> {
    None
}

#[cfg(windows)]
fn run_action(cmd: &Value) -> (bool, String) {
    let b64 = |s: &str| hbb_common::base64::encode(s.as_bytes());
    let target = cmd["target"].as_str().unwrap_or_default();
    let app = crate::get_app_name();
    let (script, timeout) = match cmd["kind"].as_str() {
        Some("uninstall") => (
            UNINSTALL_SCRIPT
                .replace("__KEY__", &b64(target))
                .replace("__SELF__", &b64(&app)),
            Duration::from_secs(900),
        ),
        Some("service") => {
            let op = match cmd["op"].as_str() {
                Some(op @ ("stop" | "start" | "restart")) => op,
                _ => return (false, "Unknown service action.".to_owned()),
            };
            (
                SERVICE_SCRIPT
                    .replace("__NAME__", &b64(target))
                    .replace("__SELF__", &b64(&app))
                    .replace("__OP__", op),
                Duration::from_secs(120),
            )
        }
        _ => return (false, "Unknown action.".to_owned()),
    };
    match crate::heritage_stats::run_powershell(&script, timeout) {
        Some(v) => (
            v["ok"].as_bool().unwrap_or(false),
            v["message"].as_str().unwrap_or_default().to_owned(),
        ),
        None => (
            false,
            "It didn't finish: it timed out or PowerShell couldn't start.".to_owned(),
        ),
    }
}

#[cfg(not(windows))]
fn run_action(_cmd: &Value) -> (bool, String) {
    let _ = Duration::from_secs(0);
    (false, "Only Windows devices support this.".to_owned())
}

#[cfg(windows)]
const SOFTWARE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$roots = @('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
           'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')
foreach ($hu in Get-ChildItem Registry::HKEY_USERS) {
  if ($hu.PSChildName -match '^S-1-5-21-[\d-]+$') {
    $roots += "Registry::HKEY_USERS\$($hu.PSChildName)\Software\Microsoft\Windows\CurrentVersion\Uninstall"
  }
}
$programs = @(@(foreach ($r in $roots) { Get-ChildItem $r | ForEach-Object {
  $p = Get-ItemProperty $_.PSPath
  if (-not $p.DisplayName -or $p.SystemComponent -eq 1 -or $p.ParentKeyName -or $p.ReleaseType) { return }
  $u = "$($p.UninstallString)"; $q = "$($p.QuietUninstallString)"
  $msi = ($p.WindowsInstaller -eq 1 -or $u -match '(?i)msiexec') -and $_.PSChildName -match '^\{[0-9A-Fa-f-]{36}\}$'
  [pscustomobject][ordered]@{ key = $_.Name; name = "$($p.DisplayName)".Trim(); version = "$($p.DisplayVersion)".Trim()
    publisher = "$($p.Publisher)".Trim(); installed = "$($p.InstallDate)"; size_kb = [int64]$p.EstimatedSize
    silent = [bool]($msi -or $q) }
} }) | Sort-Object name, key)
$services = @(Get-CimInstance Win32_Service | Sort-Object DisplayName | ForEach-Object {
  [ordered]@{ name = $_.Name; display = "$($_.DisplayName)"; state = "$($_.State)"; start = "$($_.StartMode)" } })
[ordered]@{ programs = $programs; services = $services } | ConvertTo-Json -Compress -Depth 4
"#;

#[cfg(windows)]
const UNINSTALL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
function Done($ok, $msg) { [ordered]@{ ok = $ok; message = $msg } | ConvertTo-Json -Compress; exit }
$key = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__KEY__'))
$self = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__SELF__'))
$entry = '^HKEY_(LOCAL_MACHINE\\SOFTWARE(\\WOW6432Node)?|USERS\\S-1-5-21-[\d-]+\\Software)\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\[^\\]+$'
if ($key -notmatch $entry) { Done $false 'That is not a program entry.' }
try { $p = Get-ItemProperty -LiteralPath "Registry::$key" } catch { Done $false 'That program is no longer installed.' }
$name = "$($p.DisplayName)"
if (($self -and $name -like "*$self*") -or $name -like '*RustDesk*') { Done $false "The help desk app can't uninstall itself." }
$guid = Split-Path $key -Leaf
$u = "$($p.UninstallString)"; $q = "$($p.QuietUninstallString)"
try {
  if (($p.WindowsInstaller -eq 1 -or $u -match '(?i)msiexec') -and $guid -match '^\{[0-9A-Fa-f-]{36}\}$') {
    $code = (Start-Process msiexec.exe -ArgumentList "/x $guid /qn /norestart" -Wait -PassThru -WindowStyle Hidden).ExitCode
    if ($code -in 0, 1605) { Done $true "Uninstalled $name." }
    if ($code -in 1641, 3010) { Done $true "Uninstalled $name. Windows needs a restart to finish." }
    Done $false "The uninstaller failed (Windows Installer error $code)."
  } elseif ($q) {
    $code = (Start-Process cmd.exe -ArgumentList "/s /c `"$q`"" -Wait -PassThru -WindowStyle Hidden).ExitCode
    if ($code -eq 0) { Done $true "Uninstalled $name." }
    Done $false "The uninstaller stopped with code $code."
  } else {
    Done $false "$name has no silent uninstaller. Remove it by hand in a session."
  }
} catch { Done $false ("The uninstaller couldn't start: " + $_.Exception.Message) }
"#;

#[cfg(windows)]
const SERVICE_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
function Done($ok, $msg) { [ordered]@{ ok = $ok; message = $msg } | ConvertTo-Json -Compress; exit }
$name = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__NAME__'))
$self = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__SELF__'))
$s = @(Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -eq $name })[0]
if (-not $s) { Done $false 'That service no longer exists.' }
if ($s.Name -eq $self -or $s.Name -like '*RustDesk*') { Done $false "The help desk service can't be changed from here; it would end every session." }
try {
  switch ('__OP__') {
    'stop' { Stop-Service -InputObject $s -Force }
    'start' { Start-Service -InputObject $s }
    'restart' { Restart-Service -InputObject $s -Force }
  }
} catch { Done $false ("Windows refused: " + $_.Exception.Message) }
$s.Refresh()
Done $true ("$($s.DisplayName) is now " + "$($s.Status)".ToLower() + ".")
"#;
