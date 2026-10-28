// Monitor enumeration and Windows Ink pen/touch injection via synthetic pointer devices.
use anyhow::{Context, Result};
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::Input::Pointer::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

use super::Monitor;
use crate::keys::Key;
use crate::protocol::{KeyAction, PenEvent, PenPhase, TouchPhase, TouchPoint};

pub type Source = u64;

/// Puts the GStreamer install on PATH before its delay-loaded DLLs are first used.
pub fn find_runtime() -> Result<()> {
    let root = std::env::var_os("GSTREAMER_1_0_ROOT_MSVC_X86_64")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"C:\Program Files\gstreamer\1.0\msvc_x86_64".into());
    let bin = root.join("bin");
    anyhow::ensure!(
        bin.join("gstreamer-1.0-0.dll").exists(),
        "GStreamer runtime not found in {}. Install the MSVC 64-bit runtime from https://gstreamer.freedesktop.org/download/",
        root.display()
    );
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&path));
    // Single-threaded here: nothing else reads the environment yet.
    unsafe { std::env::set_var("PATH", std::env::join_paths(dirs)?) };
    Ok(())
}

pub struct Display;

impl Display {
    pub fn open() -> Result<Display> {
        // Physical pixel coordinates for monitor rects and injected pointers.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        Ok(Display)
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            unsafe {
                let list = &mut *(data.0 as *mut Vec<Monitor>);
                let mut info = MONITORINFOEXW::default();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                if GetMonitorInfoW(h, &mut info as *mut _ as *mut _).as_bool() {
                    let r = info.monitorInfo.rcMonitor;
                    let device = String::from_utf16_lossy(&info.szDevice);
                    let device = device.trim_end_matches('\0').trim_start_matches("\\\\.\\");
                    let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
                    list.push(Monitor {
                        id: list.len() as u32,
                        name: format!("{device}{}", if primary { " (primary)" } else { "" }),
                        width: (r.right - r.left) as u32,
                        height: (r.bottom - r.top) as u32,
                        x: r.left,
                        y: r.top,
                        source: h.0 as u64,
                    });
                }
                true.into()
            }
        }
        let mut list: Vec<Monitor> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut list as *mut _ as isize),
            );
        }
        list
    }
}

const MAX_TOUCHES: usize = 10;

struct Contact {
    id: u32,
    point: POINT,
    flags: POINTER_FLAGS,
    primary: bool,
}

pub struct Injector {
    pen: HSYNTHETICPOINTERDEVICE,
    touch: HSYNTHETICPOINTERDEVICE,
    rect: RECT,
    pen_contact: bool,
    pen_in_range: bool,
    contacts: Vec<Contact>,
    held: Vec<u16>,
}

impl Injector {
    pub fn new() -> Result<Injector> {
        unsafe {
            let pen = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)
                .context("create pen device")?;
            let touch = CreateSyntheticPointerDevice(
                PT_TOUCH,
                MAX_TOUCHES as u32,
                POINTER_FEEDBACK_DEFAULT,
            )
            .context("create touch device")?;
            Ok(Injector {
                pen,
                touch,
                rect: RECT::default(),
                pen_contact: false,
                pen_in_range: false,
                contacts: Vec::new(),
                held: Vec::new(),
            })
        }
    }

    pub fn set_target(&mut self, m: &Monitor) {
        self.rect = RECT {
            left: m.x,
            top: m.y,
            right: m.x + m.width as i32,
            bottom: m.y + m.height as i32,
        };
    }

    fn to_screen(&self, x: f32, y: f32) -> POINT {
        let w = (self.rect.right - self.rect.left - 1) as f32;
        let h = (self.rect.bottom - self.rect.top - 1) as f32;
        POINT {
            x: self.rect.left + (x * w).round() as i32,
            y: self.rect.top + (y * h).round() as i32,
        }
    }

    pub fn pen(&mut self, e: &PenEvent) {
        let base = POINTER_FLAG_PRIMARY;
        let (flags, change) = match e.phase {
            PenPhase::Hover if self.pen_contact => return,
            PenPhase::Hover => (
                base | POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE,
                POINTER_CHANGE_NONE,
            ),
            PenPhase::Down => (
                base | POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_FIRSTBUTTON
                    | POINTER_FLAG_DOWN,
                POINTER_CHANGE_FIRSTBUTTON_DOWN,
            ),
            PenPhase::Move if !self.pen_contact => return,
            PenPhase::Move => (
                base | POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_FIRSTBUTTON
                    | POINTER_FLAG_UPDATE,
                POINTER_CHANGE_NONE,
            ),
            PenPhase::Up if !self.pen_contact => return,
            PenPhase::Up => (
                base | POINTER_FLAG_INRANGE | POINTER_FLAG_UP,
                POINTER_CHANGE_FIRSTBUTTON_UP,
            ),
            PenPhase::Leave if self.pen_contact => {
                self.pen(&PenEvent {
                    phase: PenPhase::Up,
                    ..*e
                });
                return self.pen(e);
            }
            PenPhase::Leave if !self.pen_in_range => return,
            PenPhase::Leave => (base | POINTER_FLAG_UPDATE, POINTER_CHANGE_NONE),
        };
        let contact = flags.0 & POINTER_FLAG_INCONTACT.0 != 0;
        // Windows Ink: inverted = eraser end in range, eraser = eraser end touching.
        let mut pen_flags = PEN_FLAG_NONE;
        if e.eraser && flags.0 & POINTER_FLAG_INRANGE.0 != 0 {
            pen_flags |= PEN_FLAG_INVERTED;
            if contact {
                pen_flags |= PEN_FLAG_ERASER;
            }
        }
        let info = POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerType: PT_PEN,
                pointerId: 0,
                pointerFlags: flags,
                ptPixelLocation: self.to_screen(e.x, e.y),
                ptPixelLocationRaw: self.to_screen(e.x, e.y),
                historyCount: 1,
                ButtonChangeType: change,
                ..Default::default()
            },
            penFlags: pen_flags,
            penMask: PEN_MASK_PRESSURE | PEN_MASK_ROTATION | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
            pressure: if contact {
                (e.pressure * 1024.0).round() as u32
            } else {
                0
            },
            rotation: e.twist as u32 % 360,
            tiltX: e.tilt_x.round() as i32,
            tiltY: e.tilt_y.round() as i32,
        };
        let input = POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 { penInfo: info },
        };
        match unsafe { InjectSyntheticPointerInput(self.pen, &[input]) } {
            Ok(()) => {
                self.pen_contact = contact;
                self.pen_in_range = flags.0 & POINTER_FLAG_INRANGE.0 != 0;
            }
            Err(err) => log::warn!("pen inject failed ({:?}): {err}", e.phase),
        }
    }

    pub fn touch(&mut self, points: &[TouchPoint]) {
        for p in points {
            let point = self.to_screen(p.x, p.y);
            let count = self.contacts.len();
            let existing = self.contacts.iter_mut().find(|c| c.id == p.id);
            match (p.phase, existing) {
                (TouchPhase::Down, None) if count < MAX_TOUCHES => {
                    let primary = count == 0;
                    self.contacts.push(Contact {
                        id: p.id,
                        point,
                        flags: POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                        primary,
                    });
                }
                (TouchPhase::Move | TouchPhase::Down, Some(c)) => c.point = point,
                (TouchPhase::Up, Some(c)) => {
                    c.point = point;
                    c.flags = POINTER_FLAG_UP;
                }
                _ => {}
            }
        }
        self.inject_touches();
    }

    fn inject_touches(&mut self) {
        if self.contacts.is_empty() {
            return;
        }
        // Every active contact has to be in each frame.
        let frame: Vec<POINTER_TYPE_INFO> = self
            .contacts
            .iter()
            .map(|c| {
                let mut flags = c.flags;
                if c.primary {
                    flags |= POINTER_FLAG_PRIMARY;
                }
                let info = POINTER_TOUCH_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_TOUCH,
                        pointerId: c.id % 256,
                        pointerFlags: flags,
                        ptPixelLocation: c.point,
                        ptPixelLocationRaw: c.point,
                        historyCount: 1,
                        ..Default::default()
                    },
                    touchFlags: TOUCH_FLAG_NONE,
                    touchMask: TOUCH_MASK_CONTACTAREA,
                    rcContact: RECT {
                        left: c.point.x - 2,
                        top: c.point.y - 2,
                        right: c.point.x + 2,
                        bottom: c.point.y + 2,
                    },
                    ..Default::default()
                };
                POINTER_TYPE_INFO {
                    r#type: PT_TOUCH,
                    Anonymous: POINTER_TYPE_INFO_0 { touchInfo: info },
                }
            })
            .collect();
        if let Err(err) = unsafe { InjectSyntheticPointerInput(self.touch, &frame) } {
            log::warn!("touch inject failed: {err}");
        }
        self.contacts.retain(|c| c.flags != POINTER_FLAG_UP);
        for c in &mut self.contacts {
            c.flags = POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
        }
    }

    pub fn key(&mut self, action: KeyAction, keys: &[Key]) {
        let vks: Vec<u16> = keys.iter().map(|k| k.vk).collect();
        if matches!(action, KeyAction::Down | KeyAction::Tap) {
            for &vk in &vks {
                send_key(vk, false);
                self.held.push(vk);
            }
        }
        if matches!(action, KeyAction::Up | KeyAction::Tap) {
            for &vk in vks.iter().rev() {
                send_key(vk, true);
                if let Some(i) = self.held.iter().rposition(|&h| h == vk) {
                    self.held.remove(i);
                }
            }
        }
    }

    /// Lifts the pen, all fingers and held keys, e.g. when the iPad disconnects.
    pub fn reset(&mut self) {
        if self.pen_contact || self.pen_in_range {
            self.pen(&PenEvent {
                phase: PenPhase::Leave,
                eraser: false,
                x: 0.5,
                y: 0.5,
                pressure: 0.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                twist: 0.0,
            });
        }
        for c in &mut self.contacts {
            c.flags = POINTER_FLAG_UP;
        }
        self.inject_touches();
        for vk in std::mem::take(&mut self.held).into_iter().rev() {
            send_key(vk, true);
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.reset();
        unsafe {
            DestroySyntheticPointerDevice(self.pen);
            DestroySyntheticPointerDevice(self.touch);
        }
    }
}

fn send_key(vk: u16, up: bool) {
    // Arrows, nav cluster and right-side keys need the extended flag.
    let extended = matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B);
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}
