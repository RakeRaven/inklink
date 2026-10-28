// One connected iPad: hello exchange, then input in and video out until the link drops.
use std::io;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::capture::{Capture, Crop};
use crate::keys::parse_combo;
use crate::net::Stream;
use crate::platform::{Display, Injector, Monitor, VirtualDisplay};
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
    let mut virt: Option<VirtualDisplay> = None;
    let mut target: Option<Monitor> = None;
    let mut crop = Crop {
        x: 0,
        y: 0,
        w: 1,
        h: 1,
    };
    // Fill slice centre, kept across reconfigures such as rotating the iPad.
    let mut center = (0.5, 0.5);
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
                let mut mode = cfg.mode;
                if mode == Mode::Virtual {
                    // Keep the same virtual monitor across repeated configs; recreate on resize.
                    let want = (cfg.max_width, cfg.max_height, cfg.fps);
                    if virt.as_ref().map(|v| v.size) != Some(want) {
                        virt = None;
                        match display.virtual_display(want.0, want.1, want.2) {
                            Ok(v) => virt = Some(v),
                            Err(e) => {
                                log::error!(
                                    "virtual display unavailable, mirroring instead: {e:#}"
                                );
                                let text =
                                    format!("Extra display unavailable, mirroring instead: {e:#}");
                                let _ =
                                    write_msg(&mut *writer.lock().unwrap(), NOTICE, &notice(&text));
                                mode = Mode::Fit;
                            }
                        }
                    }
                } else {
                    virt = None;
                }
                let monitor = match (&virt, mode) {
                    (Some(v), Mode::Virtual) => v.monitor.clone(),
                    _ => match monitors
                        .iter()
                        .find(|m| m.id == cfg.monitor)
                        .or(monitors.first())
                    {
                        Some(m) => m.clone(),
                        None => break Err(anyhow::anyhow!("no monitors to capture")),
                    },
                };
                crop = match mode {
                    Mode::Fill => Crop::fill(
                        &monitor,
                        cfg.max_width as f64 / cfg.max_height as f64,
                        center.0,
                        center.1,
                    ),
                    _ => Crop::full(&monitor),
                };
                injector.set_target(&monitor);
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
                match Capture::start(&monitor, crop, &cfg, sink) {
                    Ok(c) => capture = Some(c),
                    Err(e) => log::error!("capture failed: {e:#}"),
                }
                target = Some(monitor);
            }
            Ok(ClientMsg::Pan { dx, dy }) => {
                if let (Some(m), Some(c)) = (&target, &capture) {
                    // Dragging right shows what's to the left.
                    let (cx, cy) = crop.center(m);
                    crop.center_on(
                        m,
                        cx - dx as f64 * crop.w as f64 / m.width as f64,
                        cy - dy as f64 * crop.h as f64 / m.height as f64,
                    );
                    center = crop.center(m);
                    c.set_crop(crop);
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
                let Some(m) = &target else { continue };
                for mut e in events {
                    (e.x, e.y) = crop.to_monitor(m, e.x, e.y);
                    injector.pen(&e);
                }
            }
            Ok(ClientMsg::Touch(points)) => {
                log::debug!(
                    "touch {:?}",
                    points.iter().map(|p| (p.id, p.phase)).collect::<Vec<_>>()
                );
                let Some(m) = &target else { continue };
                let points: Vec<TouchPoint> = points
                    .into_iter()
                    .map(|mut p| {
                        (p.x, p.y) = crop.to_monitor(m, p.x, p.y);
                        p
                    })
                    .collect();
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
    drop(virt);
    writer.lock().unwrap().shutdown();
    result
}
