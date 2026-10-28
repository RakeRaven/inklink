// Virtual monitor through the Virtual Display Driver, sized to the iPad per session.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use windows::Win32::Foundation::POINTL;
use windows::Win32::Graphics::Gdi::*;
use windows::core::PCWSTR;

use super::Monitor;

const MARKER: &str = "<!-- managed by InkLink -->";

/// Native resolutions of current iPads, landscape; portrait is added too. The driver can't
/// reload without crashing, so every mode it might need is listed up front.
const IPAD_MODES: &[(u32, u32)] = &[
    (2752, 2064), // iPad Pro 13" (M4)
    (2732, 2048), // iPad Pro 12.9", iPad Air 13"
    (2420, 1668), // iPad Pro 11" (M4)
    (2388, 1668), // iPad Pro 11"
    (2360, 1640), // iPad Air 11", iPad (10th gen, A16)
    (2224, 1668), // iPad Pro 10.5"
    (2160, 1620), // iPad (7th-9th gen)
    (2266, 1488), // iPad mini (6th gen+)
];

/// Where the driver reads vdd_settings.xml from (registry VDDPATH, else its default).
fn config_dir() -> PathBuf {
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\MikeTheTech\VirtualDisplayDriver",
            "/v",
            "VDDPATH",
        ])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.split("REG_SZ").nth(1).map(|p| p.trim().to_owned()))
        })
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\VirtualDisplayDriver"))
}

fn marker_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("inklink").join("vdd-active"))
}

/// Writes a settings file offering every iPad mode (plus this one); returns whether it changed.
fn write_settings(width: u32, height: u32) -> Result<bool> {
    let dir = config_dir();
    let path = dir.join("vdd_settings.xml");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut modes: Vec<(u32, u32)> = IPAD_MODES
        .iter()
        .flat_map(|&(w, h)| [(w, h), (h, w)])
        .collect();
    if !modes.contains(&(width, height)) {
        modes.push((width, height));
    }
    let resolutions: String = modes
        .iter()
        .map(|(w, h)| {
            format!("        <resolution><width>{w}</width><height>{h}</height><refresh_rate>60</refresh_rate></resolution>
")
        })
        .collect();
    let xml = format!(
        "<?xml version='1.0' encoding='utf-8'?>
{MARKER}
<vdd_settings>
    <monitors><count>1</count></monitors>
    <gpu><friendlyname>default</friendlyname></gpu>
    <global><g_refresh_rate>60</g_refresh_rate><g_refresh_rate>120</g_refresh_rate></global>
    <resolutions>
{resolutions}    </resolutions>
    <options>
        <CustomEdid>false</CustomEdid>
        <PreventSpoof>false</PreventSpoof>
        <EdidCeaOverride>false</EdidCeaOverride>
        <HardwareCursor>true</HardwareCursor>
        <SDR10bit>false</SDR10bit>
        <HDRPlus>false</HDRPlus>
        <logging>false</logging>
        <debuglogging>false</debuglogging>
    </options>
</vdd_settings>
"
    );
    if old == xml {
        return Ok(false);
    }
    // Keep a copy of a config InkLink didn't write.
    if !old.is_empty() && !old.contains(MARKER) {
        let _ = std::fs::write(dir.join("vdd_settings.xml.bak"), &old);
    }
    std::fs::write(&path, xml).with_context(|| {
        format!(
            "write {} (your user needs write access to that folder)",
            path.display()
        )
    })?;
    Ok(true)
}

fn offers_mode(device: &[u16], width: u32, height: u32) -> bool {
    let mut m = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let mut i = 0;
    while unsafe {
        EnumDisplaySettingsW(
            PCWSTR(device.as_ptr()),
            ENUM_DISPLAY_SETTINGS_MODE(i),
            &mut m,
        )
    }
    .as_bool()
    {
        if (m.dmPelsWidth, m.dmPelsHeight) == (width, height) {
            return true;
        }
        i += 1;
    }
    false
}

/// The driver's GDI display name (e.g. `\.\DISPLAY9`) and whether it is attached to the desktop.
fn vdd_device() -> Option<(Vec<u16>, bool)> {
    for i in 0.. {
        let mut dev = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dev, 0) }.as_bool() {
            return None;
        }
        let what = String::from_utf16_lossy(&dev.DeviceString);
        if what.trim_end_matches('\0') == "Virtual Display Driver" {
            let len = dev.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
            let mut name = dev.DeviceName[..len].to_vec();
            name.push(0);
            return Some((
                name,
                dev.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP
                    != DISPLAY_DEVICE_STATE_FLAGS(0),
            ));
        }
    }
    None
}

/// Attaches (with a mode and position) or detaches (all zero) a display, like Display settings does.
fn set_mode(device: &[u16], width: u32, height: u32, fps: u32, x: i32) -> bool {
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION,
        dmPelsWidth: width,
        dmPelsHeight: height,
        ..Default::default()
    };
    if fps > 0 {
        mode.dmFields |= DM_DISPLAYFREQUENCY;
        mode.dmDisplayFrequency = fps;
    }
    mode.Anonymous1.Anonymous2.dmPosition = POINTL { x, y: 0 };
    unsafe {
        let staged = ChangeDisplaySettingsExW(
            PCWSTR(device.as_ptr()),
            Some(&mode),
            None,
            CDS_UPDATEREGISTRY | CDS_NORESET,
            None,
        );
        let applied = ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None);
        if staged != DISP_CHANGE_SUCCESSFUL || applied != DISP_CHANGE_SUCCESSFUL {
            let mut modes = Vec::new();
            let mut m = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            let mut i = 0;
            while EnumDisplaySettingsW(
                PCWSTR(device.as_ptr()),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut m,
            )
            .as_bool()
            {
                modes.push(format!(
                    "{}x{}@{}",
                    m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency
                ));
                i += 1;
            }
            modes.dedup();
            log::debug!(
                "set mode {width}x{height}@{fps}: staged {staged:?}, applied {applied:?}; offered {modes:?}"
            );
        }
        staged == DISP_CHANGE_SUCCESSFUL && applied == DISP_CHANGE_SUCCESSFUL
    }
}

fn detach() {
    if let Some((device, true)) = vdd_device() {
        set_mode(&device, 0, 0, 0, 0);
    }
}

/// An extended monitor that exists while this value lives.
pub struct VirtualDisplay {
    pub monitor: Monitor,
    pub size: (u32, u32, u32),
}

impl VirtualDisplay {
    pub fn create(
        width: u32,
        height: u32,
        fps: u32,
        monitors: impl Fn() -> Vec<Monitor>,
    ) -> Result<VirtualDisplay> {
        // New modes only take effect when the driver next starts.
        let changed = write_settings(width, height)?;
        let Some((device, _)) = vdd_device() else {
            bail!(
                "Virtual Display Driver isn't running. Install it from {}, or if it is installed, disable and re-enable it in Device Manager (Display adapters)",
                "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases"
            );
        };
        if !offers_mode(&device, width, height) {
            bail!(
                "the Virtual Display Driver doesn't offer {width}x{height} yet{}; disable and re-enable it in Device Manager (or reboot) so it picks the mode up",
                if changed { " (just added it)" } else { "" }
            );
        }
        if let Some(p) = marker_path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, "");
        }

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut attached = false;
        loop {
            if let Some((device, _)) = vdd_device() {
                let short = String::from_utf16_lossy(&device[..device.len() - 1]);
                let short = short.trim_start_matches(r"\\.\").to_owned();
                let current = monitors();
                let mine = current
                    .iter()
                    .find(|m| m.name.split(' ').next() == Some(short.as_str()));
                log::debug!(
                    "vdd device {short}, monitors {:?}",
                    current
                        .iter()
                        .map(|m| (&m.name, m.width, m.height))
                        .collect::<Vec<_>>()
                );
                match mine {
                    Some(m) if (m.width, m.height) == (width, height) => {
                        let mut m = m.clone();
                        m.name = format!("iPad ({width}x{height})");
                        log::info!("virtual display ready: {} at x={}", m.name, m.x);
                        return Ok(VirtualDisplay {
                            monitor: m,
                            size: (width, height, fps),
                        });
                    }
                    _ if !attached => {
                        // Right of everything else, so it never covers a real screen.
                        let right = current
                            .iter()
                            .filter(|m| m.name.split(' ').next() != Some(short.as_str()))
                            .map(|m| m.x + m.width as i32)
                            .max()
                            .unwrap_or(0);
                        attached = set_mode(&device, width, height, fps, right);
                    }
                    _ => {}
                }
            }
            if Instant::now() > deadline {
                remove();
                bail!("the virtual monitor didn't come up at {width}x{height}");
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

fn remove() {
    detach();
    if let Some(p) = marker_path() {
        let _ = std::fs::remove_file(p);
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        log::info!("detaching virtual display");
        remove();
    }
}

/// Detaches the driver's monitor when InkLink manages it but isn't using it (it can't go below one).
pub fn cleanup_stale() {
    // Make sure the driver's next start already offers every iPad mode.
    if vdd_device().is_some() {
        let _ = write_settings(IPAD_MODES[0].0, IPAD_MODES[0].1);
    }
    let managed = std::fs::read_to_string(config_dir().join("vdd_settings.xml"))
        .is_ok_and(|s| s.contains(MARKER));
    if (managed || marker_path().is_some_and(|p| p.exists()))
        && vdd_device().is_some_and(|(_, on)| on)
    {
        log::info!("detaching the idle virtual display");
        remove();
    }
}
