// Screen capture through the xdg-desktop-portal and pen/touch/keyboard through uinput.
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;

use anyhow::{Context, Result};
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{PersistMode, Session};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, PropType,
    UinputAbsSetup,
};

use super::Monitor;
use crate::keys::Key;
use crate::protocol::{KeyAction, PenEvent, PenPhase, TouchPhase, TouchPoint};

#[derive(Clone, Copy)]
pub struct Source {
    pub fd: i32,
    pub node: u32,
}

pub struct Display {
    runtime: tokio::runtime::Runtime,
    _proxy: Screencast,
    session: Session<Screencast>,
    fd: OwnedFd,
    node: u32,
    width: u32,
    height: u32,
}

fn token_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("inklink").join("portal-token")
}

impl Display {
    /// Asks the compositor for a monitor once; the restore token skips the dialog next time.
    pub fn open() -> Result<Display> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let token = std::fs::read_to_string(token_path()).ok();
        let (proxy, session, fd, node, size, new_token) = runtime.block_on(async {
            let proxy = Screencast::new().await?;
            let session = proxy.create_session(Default::default()).await?;
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(CursorMode::Embedded)
                        .set_sources(SourceType::Monitor)
                        .set_multiple(false)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked)
                        .set_restore_token(token.as_deref()),
                )
                .await?
                .response()?;
            let streams = proxy
                .start(&session, None, StartCastOptions::default())
                .await?
                .response()?;
            let stream = streams
                .streams()
                .first()
                .context("portal returned no stream")?
                .clone();
            let fd = proxy
                .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
                .await?;
            anyhow::Ok((
                proxy,
                session,
                fd,
                stream.pipe_wire_node_id(),
                stream.size(),
                streams.restore_token().map(str::to_owned),
            ))
        })?;
        if let Some(t) = new_token {
            let path = token_path();
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(path, t);
        }
        let (w, h) = size.unwrap_or((1920, 1080));
        Ok(Display {
            runtime,
            _proxy: proxy,
            session,
            fd,
            node,
            width: w.max(1) as u32,
            height: h.max(1) as u32,
        })
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        vec![Monitor {
            id: 0,
            name: "Shared screen".into(),
            width: self.width,
            height: self.height,
            x: 0,
            y: 0,
            source: Source {
                fd: self.fd.as_raw_fd(),
                node: self.node,
            },
        }]
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        let _ = self.runtime.block_on(self.session.close());
    }
}

const RANGE: i32 = 32767;
const MAX_SLOTS: usize = 10;

fn abs(code: AbsoluteAxisCode, min: i32, max: i32, res: i32) -> UinputAbsSetup {
    UinputAbsSetup::new(code, AbsInfo::new(0, min, max, 0, 0, res))
}

fn ev(kind: EventType, code: u16, value: i32) -> InputEvent {
    InputEvent::new(kind.0, code, value)
}

pub struct Injector {
    pen: VirtualDevice,
    touch: VirtualDevice,
    keyboard: VirtualDevice,
    pen_tool: Option<KeyCode>,
    pen_contact: bool,
    slots: [Option<u32>; MAX_SLOTS],
    next_tracking: i32,
    held: Vec<u16>,
}

impl Injector {
    pub fn new() -> Result<Injector> {
        let mut direct = AttributeSet::<PropType>::new();
        direct.insert(PropType::DIRECT);

        let mut pen_keys = AttributeSet::<KeyCode>::new();
        for k in [
            KeyCode::BTN_TOOL_PEN,
            KeyCode::BTN_TOOL_RUBBER,
            KeyCode::BTN_TOUCH,
            KeyCode::BTN_STYLUS,
        ] {
            pen_keys.insert(k);
        }
        let pen = VirtualDevice::builder()
            .context("open /dev/uinput (see README for permissions)")?
            .name("InkLink Pen")
            .with_properties(&direct)?
            .with_keys(&pen_keys)?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_X, 0, RANGE, 100))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_Y, 0, RANGE, 100))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_PRESSURE, 0, 4095, 0))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_TILT_X, -90, 90, 57))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_TILT_Y, -90, 90, 57))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_Z, -900, 899, 0))?
            .build()?;

        let mut touch_keys = AttributeSet::<KeyCode>::new();
        touch_keys.insert(KeyCode::BTN_TOUCH);
        let touch = VirtualDevice::builder()?
            .name("InkLink Touch")
            .with_properties(&direct)?
            .with_keys(&touch_keys)?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_X, 0, RANGE, 100))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_Y, 0, RANGE, 100))?
            .with_absolute_axis(&abs(
                AbsoluteAxisCode::ABS_MT_SLOT,
                0,
                MAX_SLOTS as i32 - 1,
                0,
            ))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 0, 65535, 0))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_MT_POSITION_X, 0, RANGE, 100))?
            .with_absolute_axis(&abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, 0, RANGE, 100))?
            .build()?;

        let mut kb_keys = AttributeSet::<KeyCode>::new();
        for code in 1..=127u16 {
            kb_keys.insert(KeyCode(code));
        }
        let keyboard = VirtualDevice::builder()?
            .name("InkLink Keyboard")
            .with_keys(&kb_keys)?
            .build()?;

        Ok(Injector {
            pen,
            touch,
            keyboard,
            pen_tool: None,
            pen_contact: false,
            slots: [None; MAX_SLOTS],
            next_tracking: 1,
            held: Vec::new(),
        })
    }

    // The compositor maps the virtual devices to an output; nothing to do here.
    pub fn set_target(&mut self, _m: &Monitor) {}

    pub fn pen(&mut self, e: &PenEvent) {
        let key = EventType::KEY;
        let a = EventType::ABSOLUTE;
        let mut out = Vec::new();
        let tool = if e.eraser {
            KeyCode::BTN_TOOL_RUBBER
        } else {
            KeyCode::BTN_TOOL_PEN
        };

        if matches!(e.phase, PenPhase::Leave | PenPhase::Up)
            || self.pen_tool.is_some_and(|t| t != tool)
        {
            if self.pen_contact {
                out.push(ev(key, KeyCode::BTN_TOUCH.0, 0));
                out.push(ev(a, AbsoluteAxisCode::ABS_PRESSURE.0, 0));
                self.pen_contact = false;
            }
        }
        if e.phase == PenPhase::Leave || self.pen_tool.is_some_and(|t| t != tool) {
            if let Some(t) = self.pen_tool.take() {
                out.push(ev(key, t.0, 0));
                // Tool changes need their own frame.
                let _ = self.pen.emit(&out);
                out.clear();
            }
        }
        if e.phase != PenPhase::Leave {
            out.push(ev(
                a,
                AbsoluteAxisCode::ABS_X.0,
                (e.x * RANGE as f32) as i32,
            ));
            out.push(ev(
                a,
                AbsoluteAxisCode::ABS_Y.0,
                (e.y * RANGE as f32) as i32,
            ));
            out.push(ev(
                a,
                AbsoluteAxisCode::ABS_TILT_X.0,
                e.tilt_x.round() as i32,
            ));
            out.push(ev(
                a,
                AbsoluteAxisCode::ABS_TILT_Y.0,
                e.tilt_y.round() as i32,
            ));
            out.push(ev(
                a,
                AbsoluteAxisCode::ABS_Z.0,
                ((e.twist * 10.0) as i32 + 900) % 1800 - 900,
            ));
            if self.pen_tool.is_none() {
                out.push(ev(key, tool.0, 1));
                self.pen_tool = Some(tool);
            }
            match e.phase {
                PenPhase::Down | PenPhase::Move => {
                    out.push(ev(
                        a,
                        AbsoluteAxisCode::ABS_PRESSURE.0,
                        (e.pressure * 4095.0) as i32,
                    ));
                    if !self.pen_contact {
                        out.push(ev(key, KeyCode::BTN_TOUCH.0, 1));
                        self.pen_contact = true;
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            if let Err(err) = self.pen.emit(&out) {
                log::warn!("pen emit failed: {err}");
            }
        }
    }

    pub fn touch(&mut self, points: &[TouchPoint]) {
        let a = EventType::ABSOLUTE;
        let mut out = Vec::new();
        for p in points {
            let slot = self.slots.iter().position(|s| *s == Some(p.id));
            let slot = match (p.phase, slot) {
                (_, Some(s)) => s,
                (TouchPhase::Down, None) => match self.slots.iter().position(Option::is_none) {
                    Some(s) => {
                        self.slots[s] = Some(p.id);
                        out.push(ev(a, AbsoluteAxisCode::ABS_MT_SLOT.0, s as i32));
                        out.push(ev(
                            a,
                            AbsoluteAxisCode::ABS_MT_TRACKING_ID.0,
                            self.next_tracking,
                        ));
                        self.next_tracking = self.next_tracking % 65535 + 1;
                        s
                    }
                    None => continue,
                },
                _ => continue,
            };
            out.push(ev(a, AbsoluteAxisCode::ABS_MT_SLOT.0, slot as i32));
            if p.phase == TouchPhase::Up {
                out.push(ev(a, AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1));
                self.slots[slot] = None;
            } else {
                let (x, y) = ((p.x * RANGE as f32) as i32, (p.y * RANGE as f32) as i32);
                out.push(ev(a, AbsoluteAxisCode::ABS_MT_POSITION_X.0, x));
                out.push(ev(a, AbsoluteAxisCode::ABS_MT_POSITION_Y.0, y));
                out.push(ev(a, AbsoluteAxisCode::ABS_X.0, x));
                out.push(ev(a, AbsoluteAxisCode::ABS_Y.0, y));
            }
        }
        let touching = self.slots.iter().any(Option::is_some);
        out.push(ev(EventType::KEY, KeyCode::BTN_TOUCH.0, touching as i32));
        if let Err(err) = self.touch.emit(&out) {
            log::warn!("touch emit failed: {err}");
        }
    }

    pub fn key(&mut self, action: KeyAction, keys: &[Key]) {
        let codes: Vec<u16> = keys.iter().map(|k| k.evdev).collect();
        if matches!(action, KeyAction::Down | KeyAction::Tap) {
            for &c in &codes {
                self.emit_key(c, 1);
                self.held.push(c);
            }
        }
        if matches!(action, KeyAction::Up | KeyAction::Tap) {
            for &c in codes.iter().rev() {
                self.emit_key(c, 0);
                if let Some(i) = self.held.iter().rposition(|&h| h == c) {
                    self.held.remove(i);
                }
            }
        }
    }

    fn emit_key(&mut self, code: u16, value: i32) {
        if let Err(err) = self.keyboard.emit(&[ev(EventType::KEY, code, value)]) {
            log::warn!("key emit failed: {err}");
        }
    }

    pub fn reset(&mut self) {
        self.pen(&PenEvent {
            phase: PenPhase::Leave,
            eraser: false,
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            twist: 0.0,
        });
        let lifted: Vec<TouchPoint> = self
            .slots
            .iter()
            .flatten()
            .map(|&id| TouchPoint {
                id,
                phase: TouchPhase::Up,
                x: 0.0,
                y: 0.0,
            })
            .collect();
        if !lifted.is_empty() {
            self.touch(&lifted);
        }
        for c in std::mem::take(&mut self.held).into_iter().rev() {
            self.emit_key(c, 0);
        }
    }
}
