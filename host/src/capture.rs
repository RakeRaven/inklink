// Screen capture -> hardware H.264 -> access units handed to a callback.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use crate::platform::Monitor;
use crate::protocol::StreamConfig;

pub type FrameSink = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

pub struct Capture {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    /// The element that crops: the capture source on Windows, videocrop on Linux.
    cropper: gst::Element,
    monitor: (u32, u32),
}

/// Part of a monitor to stream, in the monitor's own pixel units.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Crop {
    pub fn full(m: &Monitor) -> Crop {
        Crop {
            x: 0,
            y: 0,
            w: m.width,
            h: m.height,
        }
    }

    /// The largest slice of `m` with the given aspect ratio, centred on (cx, cy) in 0..1.
    pub fn fill(m: &Monitor, aspect: f64, cx: f64, cy: f64) -> Crop {
        let (mw, mh) = (m.width as f64, m.height as f64);
        let (w, h) = if mw / mh > aspect {
            (mh * aspect, mh)
        } else {
            (mw, mw / aspect)
        };
        let (w, h) = (
            (w.round() as u32 & !1).max(16),
            (h.round() as u32 & !1).max(16),
        );
        let mut c = Crop {
            x: 0,
            y: 0,
            w: w.min(m.width),
            h: h.min(m.height),
        };
        c.center_on(m, cx, cy);
        c
    }

    pub fn center_on(&mut self, m: &Monitor, cx: f64, cy: f64) {
        let x = cx * m.width as f64 - self.w as f64 / 2.0;
        let y = cy * m.height as f64 - self.h as f64 / 2.0;
        self.x = x.clamp(0.0, (m.width - self.w) as f64).round() as u32;
        self.y = y.clamp(0.0, (m.height - self.h) as f64).round() as u32;
    }

    pub fn center(&self, m: &Monitor) -> (f64, f64) {
        (
            (self.x as f64 + self.w as f64 / 2.0) / m.width as f64,
            (self.y as f64 + self.h as f64 / 2.0) / m.height as f64,
        )
    }

    /// Maps a 0..1 point in the streamed picture to 0..1 on the whole monitor.
    pub fn to_monitor(&self, m: &Monitor, x: f32, y: f32) -> (f32, f32) {
        (
            (self.x as f32 + x * self.w as f32) / m.width as f32,
            (self.y as f32 + y * self.h as f32) / m.height as f32,
        )
    }
}

#[cfg(windows)]
const ENCODERS: &[&str] = &[
    "nvd3d11h264enc",
    "nvautogpuh264enc",
    "qsvh264enc",
    "mfh264enc",
    "x264enc",
];
#[cfg(target_os = "linux")]
const ENCODERS: &[&str] = &["vah264lpenc", "vah264enc", "vaapih264enc", "x264enc"];

/// Fits the crop's aspect ratio inside the iPad's resolution, even dimensions.
fn output_size(c: Crop, cfg: &StreamConfig) -> (u32, u32) {
    // Portal sizes are logical, so allow scaling up to HiDPI there.
    let cap = if cfg!(windows) { 1.0 } else { 2.0 };
    let scale = (cfg.max_width as f64 / c.w as f64)
        .min(cfg.max_height as f64 / c.h as f64)
        .min(cap);
    let even = |v: f64| ((v.round() as u32) & !1).max(16);
    (even(c.w as f64 * scale), even(c.h as f64 * scale))
}

fn make(factory: &str) -> Result<gst::Element> {
    Ok(gst::ElementFactory::make(factory).build()?)
}

/// Sets a property from a string only if the element has it and the value parses.
fn try_set(e: &gst::Element, name: &str, value: &str) -> bool {
    let Some(pspec) = e.find_property(name) else {
        return false;
    };
    match gst::glib::Value::deserialize(value, pspec.value_type()) {
        Ok(v) => {
            e.set_property_from_value(name, &v);
            true
        }
        Err(_) => false,
    }
}

fn configure_encoder(enc: &gst::Element, cfg: &StreamConfig) {
    let kbps = cfg.kbps.to_string();
    let gop = (cfg.fps * 5).to_string();
    let name = enc
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_default();
    match name.as_str() {
        "x264enc" => {
            try_set(enc, "tune", "zerolatency");
            try_set(enc, "speed-preset", "superfast");
            try_set(enc, "bitrate", &kbps);
            try_set(enc, "key-int-max", &gop);
            try_set(enc, "bframes", "0");
        }
        "vaapih264enc" => {
            try_set(enc, "rate-control", "cbr");
            try_set(enc, "bitrate", &kbps);
            try_set(enc, "keyframe-period", &gop);
            try_set(enc, "max-bframes", "0");
        }
        _ => {
            // nvcodec, qsv, mediafoundation and va share most property names.
            if !try_set(enc, "preset", "p1") {
                try_set(enc, "preset", "low-latency-hp");
            }
            try_set(enc, "tune", "ultra-low-latency");
            try_set(enc, "zerolatency", "true");
            try_set(enc, "low-latency", "true");
            try_set(enc, "rc-mode", "cbr");
            try_set(enc, "rate-control", "cbr");
            try_set(enc, "bitrate", &kbps);
            try_set(enc, "gop-size", &gop);
            try_set(enc, "key-int-max", &gop);
            try_set(enc, "bframes", "0");
            try_set(enc, "b-frames", "0");
            try_set(enc, "target-usage", "7");
        }
    }
}

/// Capture source through to raw frames of the given size the encoder can take.
#[cfg(windows)]
fn source_chain(
    m: &Monitor,
    cfg: &StreamConfig,
    encoder: &str,
    (w, h): (u32, u32),
) -> Result<Vec<gst::Element>> {
    let src = make("d3d11screencapturesrc")?;
    src.set_property("monitor-handle", m.source);
    try_set(&src, "show-cursor", "true");
    let rate = gst::Caps::builder("video/x-raw")
        .features(["memory:D3D11Memory"])
        .field("framerate", gst::Fraction::new(cfg.fps as i32, 1))
        .build();
    let scaled = gst::Caps::builder("video/x-raw")
        .features(["memory:D3D11Memory"])
        .field("format", "NV12")
        .field("width", w as i32)
        .field("height", h as i32)
        .build();
    let mut chain = vec![
        src,
        capsfilter(rate)?,
        make("d3d11convert")?,
        capsfilter(scaled)?,
    ];
    if encoder == "x264enc" {
        chain.push(make("d3d11download")?);
    }
    Ok(chain)
}

#[cfg(target_os = "linux")]
fn source_chain(
    m: &Monitor,
    _cfg: &StreamConfig,
    _encoder: &str,
    (w, h): (u32, u32),
) -> Result<Vec<gst::Element>> {
    let src = make("pipewiresrc")?;
    src.set_property("fd", m.source.fd);
    src.set_property("path", m.source.node.to_string());
    try_set(&src, "do-timestamp", "true");
    // Repeat the last frame on a static screen so the iPad never starves.
    try_set(&src, "keepalive-time", "500");
    let scaled = gst::Caps::builder("video/x-raw")
        .field("format", "NV12")
        .field("width", w as i32)
        .field("height", h as i32)
        .build();
    Ok(vec![
        src,
        make("videoconvert")?,
        make("videocrop")?,
        make("videoscale")?,
        capsfilter(scaled)?,
    ])
}

fn capsfilter(caps: gst::Caps) -> Result<gst::Element> {
    Ok(gst::ElementFactory::make("capsfilter")
        .property("caps", caps)
        .build()?)
}

impl Capture {
    /// Tries each available encoder until one produces a frame.
    pub fn start(m: &Monitor, crop: Crop, cfg: &StreamConfig, out: FrameSink) -> Result<Capture> {
        let size = output_size(crop, cfg);
        for &encoder in ENCODERS {
            if gst::ElementFactory::find(encoder).is_none() {
                continue;
            }
            match Self::try_start(m, crop, cfg, encoder, size, out.clone()) {
                Ok(c) => {
                    log::info!(
                        "streaming {} at {}x{} {}fps {}kbps via {encoder}",
                        m.name,
                        size.0,
                        size.1,
                        cfg.fps,
                        cfg.kbps
                    );
                    return Ok(c);
                }
                Err(e) => log::warn!("{encoder}: {e:#}"),
            }
        }
        bail!("no working H.264 encoder")
    }

    fn try_start(
        m: &Monitor,
        crop: Crop,
        cfg: &StreamConfig,
        encoder: &str,
        size: (u32, u32),
        out: FrameSink,
    ) -> Result<Capture> {
        let pipeline = gst::Pipeline::new();
        let enc = make(encoder)?;
        configure_encoder(&enc, cfg);
        let parse = gst::ElementFactory::make("h264parse")
            .property("config-interval", -1i32)
            .build()?;
        let h264 = gst::Caps::builder("video/x-h264")
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build();
        let sink = gst_app::AppSink::builder()
            .sync(false)
            .max_buffers(2)
            .build();

        let got_frame = Arc::new(AtomicBool::new(false));
        let flag = got_frame.clone();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    flag.store(true, Ordering::Relaxed);
                    // Blocking here is the backpressure: a slow link lowers the frame rate, not latency.
                    if out(map.as_slice()) {
                        Ok(gst::FlowSuccess::Ok)
                    } else {
                        Err(gst::FlowError::Eos)
                    }
                })
                .build(),
        );

        let mut chain = source_chain(m, cfg, encoder, size)?;
        let cropper = if cfg!(windows) {
            chain[0].clone()
        } else {
            chain[2].clone()
        };
        chain.extend([enc, parse, capsfilter(h264)?, sink.clone().upcast()]);
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain)?;

        let capture = Capture {
            pipeline,
            sink,
            cropper,
            monitor: (m.width, m.height),
        };
        capture.set_crop(crop);
        capture.pipeline.set_state(gst::State::Playing)?;
        let bus = capture.pipeline.bus().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !got_frame.load(Ordering::Relaxed) {
            if Instant::now() > deadline {
                bail!("no frame within 5s");
            }
            if let Some(msg) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(50),
                &[gst::MessageType::Error],
            ) {
                if let gst::MessageView::Error(err) = msg.view() {
                    bail!("{} ({:?})", err.error(), err.debug());
                }
            }
        }
        // The real buffer size is only known now, so place the crop again.
        capture.set_crop(crop);
        Ok(capture)
    }

    /// Moves or resizes the streamed area while running.
    pub fn set_crop(&self, c: Crop) {
        let full = (c.w, c.h) == self.monitor;
        #[cfg(windows)]
        {
            let (x, y, w, h) = if full {
                (0, 0, 0, 0)
            } else {
                (c.x, c.y, c.w, c.h)
            };
            self.cropper.set_property("crop-x", x);
            self.cropper.set_property("crop-y", y);
            self.cropper.set_property("crop-width", w);
            self.cropper.set_property("crop-height", h);
        }
        #[cfg(target_os = "linux")]
        {
            // Portal sizes can be logical; scale to the buffers pipewire actually delivers.
            let caps = self
                .cropper
                .static_pad("sink")
                .and_then(|p| p.current_caps());
            let size = caps
                .as_ref()
                .and_then(|c| gst_video::VideoInfo::from_caps(c).ok());
            let (bw, bh) = size
                .map(|i| (i.width(), i.height()))
                .unwrap_or(self.monitor);
            let sx = bw as f64 / self.monitor.0 as f64;
            let sy = bh as f64 / self.monitor.1 as f64;
            let (left, top) = ((c.x as f64 * sx) as i32, (c.y as f64 * sy) as i32);
            let (w, h) = ((c.w as f64 * sx) as i32, (c.h as f64 * sy) as i32);
            let (right, bottom) = if full {
                (0, 0)
            } else {
                (bw as i32 - left - w, bh as i32 - top - h)
            };
            let (left, top) = if full { (0, 0) } else { (left, top) };
            self.cropper.set_property("left", left);
            self.cropper.set_property("top", top);
            self.cropper.set_property("right", right.max(0));
            self.cropper.set_property("bottom", bottom.max(0));
        }
    }

    pub fn force_keyframe(&self) {
        let event = gst_video::UpstreamForceKeyUnitEvent::builder()
            .all_headers(true)
            .build();
        self.sink.send_event(event);
    }

    /// Pipeline errors after startup, e.g. a display mode change.
    pub fn failed(&self) -> Option<String> {
        let msg = self
            .pipeline
            .bus()?
            .pop_filtered(&[gst::MessageType::Error])?;
        match msg.view() {
            gst::MessageView::Error(err) => Some(err.error().to_string()),
            _ => None,
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(w: u32, h: u32) -> Monitor {
        Monitor {
            id: 0,
            name: String::new(),
            width: w,
            height: h,
            x: 0,
            y: 0,
            source: Default::default(),
        }
    }

    #[test]
    fn fill_crop_matches_ipad_shape_and_clamps() {
        let m = monitor(2560, 1440);
        let mut c = Crop::fill(&m, 2388.0 / 1668.0, 0.5, 0.5);
        assert_eq!((c.w, c.h, c.x, c.y), (2062, 1440, 249, 0));
        c.center_on(&m, -5.0, 0.5);
        assert_eq!(c.x, 0);
        c.center_on(&m, 5.0, 0.5);
        assert_eq!(c.x, 2560 - 2062);
        assert_eq!(
            c.to_monitor(&m, 0.0, 0.5),
            ((2560.0 - 2062.0) / 2560.0, 0.5)
        );
    }

    #[test]
    fn output_never_upscales_on_windows() {
        let m = monitor(1920, 1080);
        let cfg = StreamConfig {
            monitor: 0,
            fps: 60,
            kbps: 1,
            max_width: 2388,
            max_height: 1668,
            mode: crate::protocol::Mode::Fit,
        };
        let (w, h) = output_size(Crop::full(&m), &cfg);
        if cfg!(windows) {
            assert_eq!((w, h), (1920, 1080));
        }
    }
}
