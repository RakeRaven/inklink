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

/// Fits the monitor's aspect ratio inside the iPad's resolution, even dimensions.
fn output_size(m: &Monitor, cfg: &StreamConfig) -> (u32, u32) {
    // Portal sizes are logical, so allow scaling up to HiDPI there.
    let cap = if cfg!(windows) { 1.0 } else { 2.0 };
    let scale = (cfg.max_width as f64 / m.width as f64)
        .min(cfg.max_height as f64 / m.height as f64)
        .min(cap);
    let even = |v: f64| ((v.round() as u32) & !1).max(16);
    (even(m.width as f64 * scale), even(m.height as f64 * scale))
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
    pub fn start(m: &Monitor, cfg: &StreamConfig, out: FrameSink) -> Result<Capture> {
        let size = output_size(m, cfg);
        for &encoder in ENCODERS {
            if gst::ElementFactory::find(encoder).is_none() {
                continue;
            }
            match Self::try_start(m, cfg, encoder, size, out.clone()) {
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
        chain.extend([enc, parse, capsfilter(h264)?, sink.clone().upcast()]);
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain)?;

        let capture = Capture { pipeline, sink };
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
        Ok(capture)
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
