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

fn main() -> Result<()> {
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

    gstreamer::init().context("GStreamer runtime not found (see README)")?;
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
    if let Ok(devices) = net::usb_devices() {
        for d in devices {
            match net::usb_connect(d) {
                Ok(s) => return Some(("USB".into(), s)),
                Err(e) => log::debug!("usb device {d}: {e}"),
            }
        }
    }
    let addrs = match fixed {
        Some(a) => vec![a],
        None => bonjour.addresses(),
    };
    for a in addrs {
        match net::connect_tcp(a) {
            Ok(s) => return Some((format!("Wi-Fi ({a})"), s)),
            Err(e) => log::debug!("{a}: {e}"),
        }
    }
    None
}
