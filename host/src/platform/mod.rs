#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{Display, Injector, Source, find_runtime};
#[cfg(windows)]
pub use windows::{Display, Injector, Source, find_runtime};

pub struct Monitor {
    pub id: u32,
    pub name: String,
    /// Physical pixels on Windows, compositor size on Linux.
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub source: Source,
}
