// One connected iPad: hello exchange, then input in and video out until the link drops.
use std::io;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::capture::Capture;
use crate::keys::parse_combo;
use crate::net::Stream;
use crate::platform::{Display, Injector};
use crate::protocol::*;

pub fn run(stream: Stream, display: &Display, injector: &mut Injector) -> Result<()> {
    stream.prepare()?;
    let mut reader = stream.try_clone()?;
    let writer = Arc::new(Mutex::new(stream));

    let (kind, payload) = read_msg(&mut reader)?;
    match parse_client(kind, &payload)? {
        ClientMsg::Hello { version } if version == VERSION => {}
        ClientMsg::Hello { version } => {
            bail!("iPad app speaks protocol {version}, host speaks {VERSION}")
        }
        _ => bail!("expected hello"),
    }

    let monitors = display.monitors();
    let infos: Vec<MonitorInfo> = monitors
        .iter()
        .map(|m| MonitorInfo {
            id: m.id,
            name: &m.name,
            width: m.width,
            height: m.height,
        })
        .collect();
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Computer".into());
    write_msg(
        &mut *writer.lock().unwrap(),
        HOST_HELLO,
        &host_hello(&host, &infos),
    )?;

    let mut capture: Option<Capture> = None;
    let result = loop {
        let (kind, payload) = match read_msg(&mut reader) {
            Ok(m) => m,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                ) =>
            {
                break Ok(());
            }
            Err(e) => break Err(e.into()),
        };
        if let Some(err) = capture.as_ref().and_then(Capture::failed) {
            log::warn!("capture stopped: {err}");
            capture = None;
        }
        match parse_client(kind, &payload) {
            Ok(ClientMsg::Config(cfg)) => {
                capture = None;
                let Some(monitor) = monitors
                    .iter()
                    .find(|m| m.id == cfg.monitor)
                    .or(monitors.first())
                else {
                    break Err(anyhow::anyhow!("no monitors to capture"));
                };
                injector.set_target(monitor);
                let w = writer.clone();
                let sink = Arc::new(move |au: &[u8]| {
                    let mut s = w.lock().unwrap();
                    let ok = write_msg(&mut *s, VIDEO, au).is_ok();
                    if !ok {
                        // Unblocks the reader so the session ends.
                        s.shutdown();
                    }
                    ok
                });
                match Capture::start(monitor, &cfg, sink) {
                    Ok(c) => capture = Some(c),
                    Err(e) => log::error!("capture failed: {e:#}"),
                }
            }
            Ok(ClientMsg::Keyframe) => {
                if let Some(c) = &capture {
                    c.force_keyframe();
                }
            }
            Ok(ClientMsg::Pen(events)) => {
                if let Some(e) = events.first() {
                    log::debug!(
                        "pen {:?} x{} at {:.3},{:.3} p{:.2}",
                        e.phase,
                        events.len(),
                        e.x,
                        e.y,
                        e.pressure
                    );
                }
                events.iter().for_each(|e| injector.pen(e))
            }
            Ok(ClientMsg::Touch(points)) => {
                log::debug!(
                    "touch {:?}",
                    points.iter().map(|p| (p.id, p.phase)).collect::<Vec<_>>()
                );
                injector.touch(&points)
            }
            Ok(ClientMsg::Key { action, combo }) => {
                log::debug!("key {action:?} {combo}");
                injector.key(action, &parse_combo(&combo))
            }
            Ok(ClientMsg::Hello { .. } | ClientMsg::Unknown) => {}
            Err(e) => log::warn!("bad message {kind:#x}: {e}"),
        }
    };
    injector.reset();
    drop(capture);
    writer.lock().unwrap().shutdown();
    result
}
