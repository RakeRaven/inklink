// Finding and connecting to the iPad: usbmuxd tunnel over USB, Bonjour or a fixed address over Wi-Fi.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use plist::{Dictionary, Value};

use crate::protocol::{PORT, SERVICE};

pub enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(match self {
            Stream::Tcp(s) => Stream::Tcp(s.try_clone()?),
            #[cfg(unix)]
            Stream::Unix(s) => Stream::Unix(s.try_clone()?),
        })
    }

    pub fn shutdown(&self) {
        let _ = match self {
            Stream::Tcp(s) => s.shutdown(Shutdown::Both),
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(Shutdown::Both),
        };
    }

    pub fn prepare(&self) -> io::Result<()> {
        let timeout = Some(Duration::from_secs(5));
        match self {
            Stream::Tcp(s) => {
                s.set_nodelay(true)?;
                s.set_write_timeout(timeout)
            }
            #[cfg(unix)]
            Stream::Unix(s) => s.set_write_timeout(timeout),
        }
    }

    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_read_timeout(t),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(t),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
        }
    }
}

pub fn connect_tcp(addr: SocketAddr) -> io::Result<Stream> {
    Ok(Stream::Tcp(TcpStream::connect_timeout(
        &addr,
        Duration::from_millis(800),
    )?))
}

fn usbmuxd() -> io::Result<Stream> {
    #[cfg(windows)]
    {
        // Apple Mobile Device Service (installed with iTunes or the Apple Devices app).
        connect_tcp(SocketAddr::from(([127, 0, 0, 1], 27015)))
    }
    #[cfg(unix)]
    {
        Ok(Stream::Unix(std::os::unix::net::UnixStream::connect(
            "/var/run/usbmuxd",
        )?))
    }
}

fn usbmux_request(s: &mut Stream, message_type: &str, extra: Dictionary) -> io::Result<Dictionary> {
    let mut dict = extra;
    dict.insert("MessageType".into(), message_type.into());
    dict.insert("ProgName".into(), "inklink".into());
    dict.insert("ClientVersionString".into(), "inklink".into());
    let mut body = Vec::new();
    Value::Dictionary(dict)
        .to_writer_xml(&mut body)
        .map_err(io::Error::other)?;

    let mut packet = Vec::with_capacity(16 + body.len());
    for v in [16 + body.len() as u32, 1, 8, 1] {
        packet.extend_from_slice(&v.to_le_bytes());
    }
    packet.extend_from_slice(&body);
    s.write_all(&packet)?;

    let mut head = [0u8; 16];
    s.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[0..4].try_into().unwrap()) as usize;
    if !(16..=1 << 20).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad usbmux reply",
        ));
    }
    let mut reply = vec![0u8; len - 16];
    s.read_exact(&mut reply)?;
    Value::from_reader(io::Cursor::new(reply))
        .map_err(io::Error::other)?
        .into_dictionary()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "usbmux reply is not a dict"))
}

/// Device IDs of iPads/iPhones attached over USB.
pub fn usb_devices() -> io::Result<Vec<u64>> {
    let mut s = usbmuxd()?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    let reply = usbmux_request(&mut s, "ListDevices", Dictionary::new())?;
    let list = reply
        .get("DeviceList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(Value::as_dictionary)
        .filter(|d| {
            d.get("Properties")
                .and_then(Value::as_dictionary)
                .and_then(|p| p.get("ConnectionType"))
                .and_then(Value::as_string)
                == Some("USB")
        })
        .filter_map(|d| d.get("DeviceID").and_then(Value::as_unsigned_integer))
        .collect())
}

/// Opens a tunnel to `PORT` on the device; fails if the app isn't listening.
pub fn usb_connect(device: u64) -> io::Result<Stream> {
    let mut s = usbmuxd()?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut d = Dictionary::new();
    d.insert("DeviceID".into(), device.into());
    d.insert("PortNumber".into(), (PORT.swap_bytes() as u64).into());
    let reply = usbmux_request(&mut s, "Connect", d)?;
    match reply.get("Number").and_then(Value::as_unsigned_integer) {
        Some(0) => {
            s.set_read_timeout(None)?;
            Ok(s)
        }
        n => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("usbmux connect result {n:?}"),
        )),
    }
}

/// Keeps a live list of iPads advertising the service on the local network.
pub struct Bonjour {
    found: Arc<Mutex<HashMap<String, SocketAddr>>>,
    _daemon: Option<mdns_sd::ServiceDaemon>,
}

impl Bonjour {
    pub fn start() -> Bonjour {
        let found = Arc::new(Mutex::new(HashMap::new()));
        let daemon = mdns_sd::ServiceDaemon::new()
            .map_err(|e| log::warn!("Bonjour unavailable: {e}"))
            .ok();
        if let Some(daemon) = daemon.clone() {
            let found = found.clone();
            std::thread::spawn(move || {
                // Re-browse regularly: a sleeping iPad drops off and only answers fresh queries.
                // Addresses are kept after the iPad goes away so a woken iPad is retried right away.
                loop {
                    let Ok(rx) = daemon.browse(SERVICE) else {
                        return;
                    };
                    let until = std::time::Instant::now() + Duration::from_secs(10);
                    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
                        match rx.recv_timeout(left) {
                            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                                if let Some(ip) = info.get_addresses_v4().into_iter().next() {
                                    let addr = SocketAddr::from((ip, info.get_port()));
                                    let mut map = found.lock().unwrap();
                                    if map.insert(info.fullname.clone(), addr) != Some(addr) {
                                        log::info!("found {} at {addr}", info.fullname);
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    let _ = daemon.stop_browse(SERVICE);
                }
            });
        }
        Bonjour {
            found,
            _daemon: daemon,
        }
    }

    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.found.lock().unwrap().values().copied().collect()
    }
}
