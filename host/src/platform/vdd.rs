// Virtual monitor through the Virtual Display Driver, sized to the iPad per session.
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::Monitor;

const PIPE: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
const MARKER: &str = "<!-- managed by InkLink -->";

fn pipe_send(cmd: &str) -> std::io::Result<()> {
    let mut pipe = std::fs::OpenOptions::new().write(true).open(PIPE)?;
    let wide: Vec<u8> = cmd.encode_utf16().flat_map(u16::to_le_bytes).collect();
    pipe.write_all(&wide)
}

fn driver_running() -> bool {
    std::fs::metadata(PIPE).is_ok()
}

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

fn write_settings(count: u32, width: u32, height: u32, fps: u32) -> Result<()> {
    let dir = config_dir();
    let path = dir.join("vdd_settings.xml");
    // Keep a copy of a config InkLink didn't write.
    if let Ok(old) = std::fs::read_to_string(&path) {
        if !old.contains(MARKER) {
            let _ = std::fs::write(dir.join("vdd_settings.xml.bak"), old);
        }
    }
    let xml = format!(
        "<?xml version='1.0' encoding='utf-8'?>
{MARKER}
<vdd_settings>
    <monitors><count>{count}</count></monitors>
    <gpu><friendlyname>default</friendlyname></gpu>
    <global><g_refresh_rate>{fps}</g_refresh_rate></global>
    <resolutions>
        <resolution><width>{width}</width><height>{height}</height><refresh_rate>{fps}</refresh_rate></resolution>
    </resolutions>
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
    std::fs::write(&path, xml).with_context(|| {
        format!(
            "write {} (your user needs write access to that folder)",
            path.display()
        )
    })
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
        if !driver_running() {
            bail!(
                "Virtual Display Driver isn't installed. Get it from                  https://github.com/VirtualDrivers/Virtual-Display-Driver/releases, then give your user                  write access to {} so InkLink can set the iPad's resolution",
                config_dir().display()
            );
        }
        let before: Vec<u64> = monitors().iter().map(|m| m.source).collect();
        write_settings(1, width, height, fps)?;
        if let Some(p) = marker_path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, "");
        }
        // Reloads the driver, which re-reads the file.
        pipe_send("SETDISPLAYCOUNT 1").context("talk to the Virtual Display Driver")?;

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let mut fresh: Vec<Monitor> = monitors()
                .into_iter()
                .filter(|m| !before.contains(&m.source))
                .collect();
            fresh.sort_by_key(|m| (m.width, m.height) != (width, height));
            if let Some(mut m) = fresh.into_iter().next() {
                m.name = format!("iPad ({}x{})", m.width, m.height);
                log::info!("virtual display ready: {}", m.name);
                return Ok(VirtualDisplay {
                    monitor: m,
                    size: (width, height, fps),
                });
            }
            if Instant::now() > deadline {
                remove();
                bail!("the virtual monitor didn't appear");
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

fn remove() {
    let _ = pipe_send("SETDISPLAYCOUNT 0");
    if let Some(p) = marker_path() {
        let _ = std::fs::remove_file(p);
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        log::info!("removing virtual display");
        remove();
    }
}

/// Removes a virtual monitor left behind by a crash.
pub fn cleanup_stale() {
    if marker_path().is_some_and(|p| p.exists()) && driver_running() {
        log::info!("removing a virtual display left from last run");
        remove();
    }
}
