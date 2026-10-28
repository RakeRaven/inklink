// Wire format: [u8 kind][u32 LE length][payload], all numbers little-endian.
use std::io::{self, Read, Write};

pub const PORT: u16 = 47800;
pub const VERSION: u16 = 1;
pub const SERVICE: &str = "_inklink._tcp.local.";
const MAX_MSG: usize = 4 << 20;

// host -> iPad
pub const HOST_HELLO: u8 = 0x01;
pub const VIDEO: u8 = 0x02;
// iPad -> host
pub const CLIENT_HELLO: u8 = 0x10;
pub const STREAM_CONFIG: u8 = 0x11;
pub const KEYFRAME: u8 = 0x12;
pub const PEN: u8 = 0x20;
pub const TOUCH: u8 = 0x21;
pub const KEY: u8 = 0x22;

pub fn write_msg(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)
}

pub fn read_msg(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[1..5].try_into().unwrap()) as usize;
    if len > MAX_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

pub struct StreamConfig {
    pub monitor: u32,
    pub fps: u32,
    pub kbps: u32,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PenPhase {
    Hover,
    Down,
    Move,
    Up,
    Leave,
}

pub struct PenEvent {
    pub phase: PenPhase,
    pub eraser: bool,
    /// Normalized 0..1 within the streamed monitor.
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// Degrees, -90..90. Positive x leans right, positive y leans toward the user.
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// Barrel roll in degrees, 0..360.
    pub twist: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

pub struct TouchPoint {
    pub id: u32,
    pub phase: TouchPhase,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyAction {
    Up,
    Down,
    Tap,
}

pub enum ClientMsg {
    Hello { version: u16 },
    Config(StreamConfig),
    Keyframe,
    Pen(Vec<PenEvent>),
    Touch(Vec<TouchPoint>),
    Key { action: KeyAction, combo: String },
    Unknown,
}

struct Buf<'a>(&'a [u8]);

impl Buf<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short message"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        Ok(if v.is_finite() { v } else { 0.0 })
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

pub fn parse_client(kind: u8, payload: &[u8]) -> io::Result<ClientMsg> {
    let mut b = Buf(payload);
    Ok(match kind {
        CLIENT_HELLO => ClientMsg::Hello { version: b.u16()? },
        STREAM_CONFIG => ClientMsg::Config(StreamConfig {
            monitor: b.u32()?,
            fps: (b.u16()? as u32).clamp(10, 240),
            kbps: b.u32()?.clamp(1_000, 200_000),
            max_width: b.u32()?.clamp(64, 8192),
            max_height: b.u32()?.clamp(64, 8192),
        }),
        KEYFRAME => ClientMsg::Keyframe,
        PEN => {
            let mut events = Vec::new();
            while !b.0.is_empty() {
                let phase = match b.u8()? {
                    0 => PenPhase::Hover,
                    1 => PenPhase::Down,
                    2 => PenPhase::Move,
                    3 => PenPhase::Up,
                    _ => PenPhase::Leave,
                };
                let flags = b.u8()?;
                events.push(PenEvent {
                    phase,
                    eraser: flags & 1 != 0,
                    x: b.f32()?.clamp(0.0, 1.0),
                    y: b.f32()?.clamp(0.0, 1.0),
                    pressure: b.f32()?.clamp(0.0, 1.0),
                    tilt_x: b.f32()?.clamp(-90.0, 90.0),
                    tilt_y: b.f32()?.clamp(-90.0, 90.0),
                    twist: b.f32()?.rem_euclid(360.0),
                });
            }
            ClientMsg::Pen(events)
        }
        TOUCH => {
            let n = b.u8()?;
            let mut points = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let id = b.u32()?;
                let phase = match b.u8()? {
                    0 => TouchPhase::Down,
                    1 => TouchPhase::Move,
                    _ => TouchPhase::Up,
                };
                points.push(TouchPoint {
                    id,
                    phase,
                    x: b.f32()?.clamp(0.0, 1.0),
                    y: b.f32()?.clamp(0.0, 1.0),
                });
            }
            ClientMsg::Touch(points)
        }
        KEY => {
            let action = match b.u8()? {
                0 => KeyAction::Up,
                1 => KeyAction::Down,
                _ => KeyAction::Tap,
            };
            ClientMsg::Key {
                action,
                combo: b.str()?,
            }
        }
        _ => ClientMsg::Unknown,
    })
}

pub struct MonitorInfo<'a> {
    pub id: u32,
    pub name: &'a str,
    pub width: u32,
    pub height: u32,
}

pub fn host_hello(host_name: &str, monitors: &[MonitorInfo]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&VERSION.to_le_bytes());
    p.push(if cfg!(windows) { 0 } else { 1 });
    put_str(&mut p, host_name);
    p.push(monitors.len().min(255) as u8);
    for m in monitors.iter().take(255) {
        p.extend_from_slice(&m.id.to_le_bytes());
        p.extend_from_slice(&m.width.to_le_bytes());
        p.extend_from_slice(&m.height.to_le_bytes());
        put_str(&mut p, m.name);
    }
    p
}

fn put_str(p: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    p.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    p.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pen_roundtrip() {
        let mut p = vec![1u8, 1];
        for v in [0.5f32, 0.25, 0.8, 10.0, -20.0, 400.0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        let ClientMsg::Pen(ev) = parse_client(PEN, &p).unwrap() else {
            panic!()
        };
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].phase, PenPhase::Down);
        assert!(ev[0].eraser);
        assert_eq!(ev[0].twist, 40.0);
    }

    #[test]
    fn framing() {
        let mut out = Vec::new();
        write_msg(&mut out, KEYFRAME, b"abc").unwrap();
        let (k, p) = read_msg(&mut &out[..]).unwrap();
        assert_eq!((k, p.as_slice()), (KEYFRAME, &b"abc"[..]));
    }

    #[test]
    fn short_message_is_error() {
        assert!(parse_client(STREAM_CONFIG, &[1, 2]).is_err());
    }
}
