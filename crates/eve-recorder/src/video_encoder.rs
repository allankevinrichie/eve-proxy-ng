//! In-process video encoding via ffmpeg-next (libavcodec/libavformat):
//! replaces the ffmpeg.exe subprocess + stdin pipe. The linked library
//! set (BtbN 7.1 gpl-shared, nvenc SDK 11.1 headers) is what actually
//! runs — probing an encoder here tests the real DLLs, not whatever
//! ffmpeg.exe happens to be on PATH.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use ffmpeg::codec::encoder::Video as EncoderHandle;
use ffmpeg::format::context::Output;
use ffmpeg::frame::video::Video;
use ffmpeg::util::rational::Rational;

use crate::error::{RecorderError, Result};

/// Candidate chain: NVIDIA → AMD → Intel hardware, libx264 CPU fallback.
pub const ENCODER_CANDIDATES: &[(&str, &[(&str, &str)])] = &[
    ("h264_nvenc", &[("preset", "p4"), ("rc", "vbr"), ("b", "6M")]),
    ("h264_amf", &[("quality", "balanced"), ("b", "6M")]),
    ("h264_qsv", &[("preset", "veryfast"), ("b", "6M")]),
    ("libx264", &[("preset", "veryfast"), ("crf", "23")]),
];

/// Required headroom over real time for a hardware backend to be chosen.
pub const MIN_SPEED_MULTIPLE: f64 = 1.5;

fn dict_from(pairs: &[(String, String)]) -> ffmpeg::Dictionary<'static> {
    let mut dict: ffmpeg::Dictionary<'static> = ffmpeg::Dictionary::new();
    for (key, value) in pairs {
        dict.set(key, value);
    }
    dict
}

fn default_opts_for(name: &str) -> &'static [(&'static str, &'static str)] {
    ENCODER_CANDIDATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, opts)| *opts)
        .unwrap_or(&[])
}

/// Try to open an encoder at the target geometry and push a few frames
/// through it (no muxer). Returns (ok, failure reason).
pub fn probe_backend(name: &str, width: u32, height: u32, fps: u32) -> (bool, String) {
    let mut session = match BareSession::open(name, width, height, fps) {
        Ok(session) => session,
        Err(e) => return (false, e.to_string()),
    };
    match session.encode_test_frames(8) {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e.to_string()),
    }
}

/// Encode ~2 seconds of frames and return the achieved speed as a
/// multiple of real time. Availability alone is not enough: an iGPU
/// encoder can probe OK yet sustain below real time at 2K-class sizes.
pub fn throughput_multiple(name: &str, width: u32, height: u32, fps: u32) -> Option<f64> {
    let mut session = BareSession::open(name, width, height, fps).ok()?;
    let frames = (fps.max(10) * 2) as i64;
    let started = std::time::Instant::now();
    session.encode_test_frames(frames as usize).ok()?;
    let elapsed = started.elapsed().as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }
    Some(frames as f64 / elapsed / fps as f64)
}

/// Encoder session without a muxer (probe/throughput path).
struct BareSession {
    encoder: EncoderHandle,
    packet: ffmpeg::packet::Packet,
    width: u32,
    height: u32,
}

impl BareSession {
    fn open(
        name: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> std::result::Result<Self, ffmpeg::Error> {
        let codec = ffmpeg::codec::encoder::find_by_name(name)
            .ok_or(ffmpeg::Error::EncoderNotFound)?;
        let time_base = Rational::new(1, fps as i32);
        let mut enc = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|_| ffmpeg::Error::EncoderNotFound)?;
        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(ffmpeg::format::Pixel::NV12);
        enc.set_time_base(time_base);
        enc.set_frame_rate(Some(Rational::new(1, fps as i32)));
        enc.set_bit_rate(6_000_000);
        enc.set_gop(2 * fps);
        let encoder = enc
            .open_as_with(codec, dict_from(&default_opts_for(name).iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>()))?;
        Ok(Self {
            encoder,
            packet: ffmpeg::packet::Packet::empty(),
            width,
            height,
        })
    }

    fn encode_test_frames(&mut self, count: usize) -> std::result::Result<(), ffmpeg::Error> {
        let mut frame = Video::new(ffmpeg::format::Pixel::NV12, self.width.max(1), self.height.max(1));
        let stride = frame.stride(0);
        for i in 0..count as i64 {
            // Gradient payload so the encoder does real work.
            let plane = frame.data_mut(0);
            for (j, byte) in plane.iter_mut().enumerate() {
                let row = j / stride;
                let col = j % stride;
                *byte = ((i * 13 + row as i64 + col as i64) % 256) as u8;
            }
            frame.set_pts(Some(i));
            self.encoder.send_frame(&frame)?;
            while self.encoder.receive_packet(&mut self.packet).is_ok() {}
        }
        self.encoder.send_eof()?;
        while self.encoder.receive_packet(&mut self.packet).is_ok() {}
        Ok(())
    }
}

/// A live recording session: encoder + mp4 muxer.
pub struct VideoEncoder {
    encoder: EncoderHandle,
    octx: Output,
    stream_index: usize,
    frame: Video,
    next_pts: i64,
    encoder_time_base: Rational,
    stream_time_base: Rational,
    pub width: u32,
    pub height: u32,
    pub name: String,
}

impl VideoEncoder {
    /// Open `name` at the exact geometry the capture will feed (already
    /// even-dimensioned, already scaled to the target resolution).
    pub fn open(
        path: &Path,
        name: &str,
        width: u32,
        height: u32,
        fps: u32,
        opts: &[(String, String)],
    ) -> Result<Self> {
        let codec = ffmpeg::codec::encoder::find_by_name(name)
            .ok_or_else(|| RecorderError::Capture(format!("encoder {name} not found")))?;
        // ffmpeg 5+ flow: open the encoder on a bare codec context first,
        // then create the muxer stream FROM the opened context.
        let time_base = Rational::new(1, fps as i32);
        // new_with_codec seeds the context with the codec's defaults —
        // x264 rejects the codec-less variant ("broken ffmpeg default
        // settings").
        let mut enc = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|e| RecorderError::Capture(format!("video encoder: {e}")))?;
        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(ffmpeg::format::Pixel::NV12);
        enc.set_time_base(time_base);
        enc.set_frame_rate(Some(Rational::new(1, fps as i32)));
        enc.set_gop(2 * fps);
        enc.set_bit_rate(6_000_000);
        let encoder = enc
            .open_as_with(codec, dict_from(opts))
            .map_err(|e| RecorderError::Capture(format!("open {name}: {e}")))?;
        let mut octx = ffmpeg::format::output(path)
            .map_err(|e| RecorderError::Capture(format!("output open: {e}")))?;
        let mut stream = octx
            .add_stream_with(&encoder)
            .map_err(|e| RecorderError::Capture(format!("add stream: {e}")))?;
        unsafe {
            // The muxer stream runs on its own time base; store it on the
            // raw stream so rescaling below has the correct target.
            (*stream.as_mut_ptr()).time_base = time_base.into();
        }
        let stream_index = stream.index();
        drop(stream);
        let _ = octx
            .write_header()
            .map_err(|e| RecorderError::Capture(format!("write_header: {e}")))?;
        // Read the muxer's FINAL time base only after write_header — mp4
        // rewrites the stream time base there, and rescaling to the
        // pre-header value corrupts every timestamp.
        let stream_time_base = octx
            .stream(stream_index)
            .ok_or_else(|| RecorderError::Capture("stream vanished".into()))?
            .time_base();
        let frame = Video::new(ffmpeg::format::Pixel::NV12, width.max(1), height.max(1));
        Ok(Self {
            encoder,
            octx,
            stream_index,
            frame,
            next_pts: 0,
            encoder_time_base: time_base,
            stream_time_base,
            width,
            height,
            name: name.to_string(),
        })
    }

    /// Feed one compact NV12 frame: w*h Y bytes then ceil(h/2) rows of
    /// interleaved UV (each row w bytes).
    pub fn encode(&mut self, nv12: &[u8]) -> Result<()> {
        let w = self.width as usize;
        let h = self.height as usize;
        let chroma_rows = (h + 1) / 2;
        let y_stride = self.frame.stride(0);
        let uv_stride = self.frame.stride(1);
        let y_plane = self.frame.data_mut(0);
        for row in 0..h {
            let src = &nv12[row * w..(row + 1) * w];
            y_plane[row * y_stride..row * y_stride + w].copy_from_slice(src);
        }
        let uv_plane = self.frame.data_mut(1);
        for row in 0..chroma_rows {
            let src_start = w * h + row * w;
            let src = &nv12[src_start..(src_start + w).min(nv12.len())];
            uv_plane[row * uv_stride..row * uv_stride + src.len()].copy_from_slice(src);
        }
        self.frame.set_pts(Some(self.next_pts));
        self.next_pts += 1;
        self.encoder
            .send_frame(&self.frame)
            .map_err(|e| RecorderError::Capture(format!("send_frame: {e}")))?;
        self.drain()
    }

    fn drain(&mut self) -> Result<()> {
        let mut packet = ffmpeg::packet::Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(self.stream_index);
                    packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
                    packet
                        .write_interleaved(&mut self.octx)
                        .map_err(|e| RecorderError::Capture(format!("write_packet: {e}")))?;
                }
                Err(ffmpeg::Error::Eof) => break,
                // EAGAIN ("resource temporarily unavailable"): the
                // encoder needs more input before producing output —
                // a normal stop condition for this drain pass. The raw
                // errno sign varies by platform plumbing.
                Err(ffmpeg::Error::Other { errno }) if errno.abs() == 11 => break,
                Err(e) => return Err(RecorderError::Capture(format!("receive_packet: {e}"))),
            }
        }
        Ok(())
    }

    /// Flush and finalize the mp4 (writes the moov trailer).
    pub fn finish(mut self) -> Result<()> {
        self.encoder
            .send_eof()
            .map_err(|e| RecorderError::Capture(format!("send_eof: {e}")))?;
        self.drain()?;
        self.octx
            .write_trailer()
            .map_err(|e| RecorderError::Capture(format!("write_trailer: {e}")))?;
        Ok(())
    }
}

/// Pick a H.264 encoder backend: explicit name, or the first candidate
/// that opens AND (for hardware) sustains >= MIN_SPEED_MULTIPLE at the
/// capture size. Every decision is logged with its reason.
pub fn pick_encoder_backend(
    explicit: Option<&str>,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(String, Vec<(String, String)>)> {
    if let Some(name) = explicit {
        let (ok, reason) = probe_backend(name, width, height, fps);
        if ok {
            tracing::info!(encoder = name, "selected video encoder (--encoder)");
            return Ok((
                name.to_string(),
                default_opts_for(name).iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ));
        }
        tracing::error!(encoder = name, reason = %reason, "explicit encoder failed");
        return Err(RecorderError::NoEncoder);
    }
    for (name, opts) in ENCODER_CANDIDATES {
        let (ok, reason) = probe_backend(name, width, height, fps);
        if !ok {
            tracing::info!(encoder = name, reason = %reason, "encoder backend unavailable");
            continue;
        }
        let hardware = !name.starts_with("libx");
        if hardware {
            match throughput_multiple(name, width, height, fps) {
                Some(speed) if speed >= MIN_SPEED_MULTIPLE => {
                    tracing::info!(
                        encoder = name,
                        speed_multiple = speed,
                        "selected video encoder (throughput verified)"
                    );
                    return Ok((
                        (*name).to_string(),
                        opts.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                    ));
                }
                Some(speed) => {
                    tracing::info!(
                        encoder = name,
                        speed_multiple = speed,
                        "backend available but too slow for the capture size; skipping"
                    );
                    continue;
                }
                None => {
                    tracing::info!(encoder = name, "throughput probe failed; skipping");
                    continue;
                }
            }
        }
        tracing::info!(encoder = name, "selected video encoder");
        return Ok((
            (*name).to_string(),
            opts.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ));
    }
    Err(RecorderError::NoEncoder)
}
