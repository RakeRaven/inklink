mod capture;
mod keys;
mod net;
mod platform;
mod protocol;
mod session;

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};

const USAGE: &str = "inklink-host [--host <ipad-ip>]\n\n\
Waits for the InkLink iPad app over USB (needs usbmuxd / Apple Devices) or Wi-Fi (Bonjour),\n\
or connects to --host directly. Set RUST_LOG=debug for more output.";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        // Keep a double-clicked console window open long enough to read it.
        if cfg!(windows) {
            eprintln!("Press Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut fixed: Option<SocketAddr> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => {
                let ip: IpAddr = args
                    .next()
                    .context("--host needs an address")?
                    .parse()
                    .context("bad --host address")?;
                fixed = Some(SocketAddr::new(ip, protocol::PORT));
            }
            _ => {
                println!("{USAGE}");
                return Ok(());
            }
        }
    }

    platform::find_runtime()?;
    gstreamer::init().context("GStreamer failed to start")?;
    let display = platform::Display::open()?;
    let mut injector = platform::Injector::new()?;
    let bonjour = net::Bonjour::start();
    log::info!("waiting for the iPad...");

    loop {
        match connect(fixed, &bonjour) {
            Some((how, stream)) => {
                log::info!("connected over {how}");
                match session::run(stream, &display, &mut injector) {
                    Ok(()) => log::info!("iPad disconnected"),
                    Err(e) => log::warn!("session ended: {e:#}"),
                }
            }
            None => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

/// USB first (lower latency), then Wi-Fi.
fn connect(fixed: Option<SocketAddr>, bonjour: &net::Bonjour) -> Option<(String, net::Stream)> {
    // A fixed --host means exactly that address.
    if let Some(a) = fixed {
        return net::connect_tcp(a)
            .ok()
            .map(|s| (format!("Wi-Fi ({a})"), s));
    }
    if let Ok(devices) = net::usb_devices() {
        for d in devices {
            match net::usb_connect(d) {
                Ok(s) => return Some(("USB".into(), s)),
                Err(e) => log::debug!("usb device {d}: {e}"),
            }
        }
    }
    let mut addrs = bonjour.addresses();
    addrs.extend(last_ipad().filter(|l| !addrs.contains(l)));
    for a in addrs {
        match net::connect_tcp(a) {
            Ok(s) => {
                remember_ipad(a);
                return Some((format!("Wi-Fi ({a})"), s));
            }
            Err(e) => log::debug!("{a}: {e}"),
        }
    }
    None
}

fn last_ipad_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("inklink").join("last-ipad"))
}

/// Last address that worked, retried even when Bonjour has gone quiet.
fn last_ipad() -> Option<SocketAddr> {
    std::fs::read_to_string(last_ipad_path()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn remember_ipad(addr: SocketAddr) {
    if let Some(path) = last_ipad_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, addr.to_string());
    }
}
