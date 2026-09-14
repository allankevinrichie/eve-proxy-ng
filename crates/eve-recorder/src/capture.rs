//! Per-client window video capture: Windows Graphics Capture (by HWND,
//! correct even when occluded) drained at a fixed tick, scaled to the
//! configured resolution budget via swscale, watermarked with the
//! session-relative capture time, and encoded IN-PROCESS (ffmpeg-next:
//! libavcodec/libavformat linked from third_party, no ffmpeg.exe
//! subprocess and no stdin pipe).
//!
//! The capture tick and the encoder run on separate threads joined by a
//! bounded queue. If the encoder cannot keep up, frames are **dropped
//! and counted** (with a warning) — the frame-index→time mapping
//! `anchor + n / fps` stays exact, which the per-frame watermark
//! independently confirms in the pixels.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TrySendError, sync_channel};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ffmpeg_next as ffmpeg;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use eve_memory::discovery::GameClient;

use crate::clock::SharedClock;
use crate::error::{RecorderError, Result};
use crate::video_encoder::{self, VideoEncoder};
use crate::watermark::{draw_stamp_nv12_uv, draw_stamp_nv12_y};

/// Warn about encoder backlog at most every N dropped frames.
const DROP_WARN_INTERVAL: u64 = 10;

/// Target-resolution budgets (total pixel counts, aspect preserved by
/// scaling, never upscaled).
pub const BUDGET_720P: u64 = 1280 * 720;
pub const BUDGET_1080P: u64 = 1920 * 1080;

/// `--video-size` selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoSize {
    P720,
    P1080,
    Native,
}

impl VideoSize {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "720p" => Some(VideoSize::P720),
            "1080p" => Some(VideoSize::P1080),
            "native" => Some(VideoSize::Native),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VideoSize::P720 => "720p",
            VideoSize::P1080 => "1080p",
            VideoSize::Native => "native",
        }
    }
}

/// Largest even (w, h) with w*h <= budget preserving the source aspect,
/// never larger than the source. Returns (target_w, target_h, scale)
/// where scale is the exact height ratio used for coordinate mapping.
pub fn fit_budget(src_w: u32, src_h: u32, budget: Option<u64>) -> (u32, u32, f64) {
    let src_w = src_w.max(1);
    let src_h = src_h.max(1);
    let Some(budget) = budget else {
        return (src_w & !1, src_h & !1, (src_h & !1) as f64 / src_h as f64);
    };
    let pixels = src_w as u64 * src_h as u64;
    if pixels <= budget {
        return (src_w & !1, src_h & !1, (src_h & !1) as f64 / src_h as f64);
    }
    let scale = (budget as f64 / pixels as f64).sqrt();
    let mut w = ((src_w as f64 * scale).floor() as u32).max(2) & !1;
    let mut h = ((src_h as f64 * scale).floor() as u32).max(2) & !1;
    // Even rounding may nudge past the budget; shave width until inside.
    while w as u64 * h as u64 > budget && w > 2 {
        w -= 2;
    }
    if w as u64 * h as u64 > budget && h > 2 {
        h -= 2;
    }
    (w, h, h as f64 / src_h as f64)
}

#[derive(Debug, Clone)]
pub struct VideoStats {
    pub frames_written: u64,
    pub frames_dropped: u64,
    pub anchor_t_ms: Option<u64>,
    /// Output (scaled) video dimensions.
    pub width: u32,
    pub height: u32,
    /// Capture-window (native) dimensions.
    pub native_width: u32,
    pub native_height: u32,
    /// scale = output_h / native_h; map input client coords to video
    /// pixels with `video_px = (client + native_client_offset) * scale`.
    pub scale: f64,
    pub fps: u32,
    pub encoder: String,
    pub video_size: &'static str,
    pub client_offset: (i32, i32),
    pub stopped_reason: String,
    /// None while the in-process encoder ran clean; error text otherwise.
    pub encoder_error: Option<String>,
}

/// Spawn a capture thread. It initializes WGC + swscale + the in-process
/// encoder (backend picked HERE, once the window geometry is known so
/// the throughput gate runs at the real output size), reports ready,
/// then blocks until the session gate sends the shared clock.
#[allow(clippy::too_many_arguments)]
pub fn spawn_capture(
    stop: Arc<AtomicBool>,
    client: GameClient,
    fps: u32,
    video_size: VideoSize,
    explicit_encoder: Option<String>,
    out: PathBuf,
    clock_rx: Receiver<SharedClock>,
    ready_tx: Sender<&'static str>,
) -> Result<JoinHandle<VideoStats>> {
    let hwnd_raw = client.window_handle_raw().ok_or_else(|| {
        RecorderError::Capture(format!("client {} has no main window", client.pid))
    })?;
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")))?;
    let thread = std::thread::Builder::new()
        .name(format!("capture-{}", client.pid))
        .spawn(move || {
            let result = run_capture(
                stop,
                client,
                hwnd_raw,
                fps,
                video_size,
                explicit_encoder,
                out,
                clock_rx,
                ready_tx,
            );
            match result {
                Ok(stats) => stats,
                Err(e) => VideoStats {
                    frames_written: 0,
                    frames_dropped: 0,
                    anchor_t_ms: None,
                    width: 0,
                    height: 0,
                    native_width: 0,
                    native_height: 0,
                    scale: 1.0,
                    fps,
                    encoder: String::new(),
                    video_size: video_size.as_str(),
                    client_offset: (0, 0),
                    stopped_reason: format!("capture_error: {e}"),
                    encoder_error: None,
                },
            }
        })
        .expect("spawn capture thread");
    Ok(thread)
}

/// One frame handed from the capture tick to the encoder feeder.
struct Frame {
    nv12: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn run_capture(
    stop: Arc<AtomicBool>,
    client: GameClient,
    hwnd_raw: isize,
    fps: u32,
    video_size: VideoSize,
    explicit_encoder: Option<String>,
    out: PathBuf,
    clock_rx: Receiver<SharedClock>,
    ready_tx: Sender<&'static str>,
) -> Result<VideoStats> {
    let budget = match video_size {
        VideoSize::P720 => Some(BUDGET_720P),
        VideoSize::P1080 => Some(BUDGET_1080P),
        VideoSize::Native => None,
    };
    let native_client_offset = window_client_offset(hwnd_raw);
    let mut stats = VideoStats {
        frames_written: 0,
        frames_dropped: 0,
        anchor_t_ms: None,
        width: 0,
        height: 0,
        native_width: 0,
        native_height: 0,
        scale: 1.0,
        fps,
        encoder: String::new(),
        video_size: video_size.as_str(),
        client_offset: native_client_offset,
        stopped_reason: String::new(),
        encoder_error: None,
    };
    let hwnd = HWND(hwnd_raw as *mut _);

    unsafe {
        // --- D3D11 device (shared by WGC and the staging readback) ---
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            7, // D3D11_SDK_VERSION
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| RecorderError::Capture(format!("D3D11CreateDevice: {e}")))?;
        let device = device.unwrap();
        let context = context.unwrap();

        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|e| RecorderError::Capture(format!("cast IDXGIDevice: {e}")))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
            .map_err(|e| RecorderError::Capture(format!("CreateDirect3D11DeviceFromDXGIDevice: {e}")))?;
        let d3d_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| RecorderError::Capture(format!("cast IDirect3DDevice: {e}")))?;

        // --- GraphicsCaptureItem from the client HWND ---
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<windows::Graphics::Capture::GraphicsCaptureItem, _>()
                .map_err(|e| RecorderError::Capture(format!("capture interop factory: {e}")))?;
        let item: windows::Graphics::Capture::GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .map_err(|e| RecorderError::Capture(format!("CreateForWindow: {e}")))?;
        let size = item
            .Size()
            .map_err(|e| RecorderError::Capture(format!("item.Size: {e}")))?;
        let src_w = size.Width.max(1) as u32;
        let src_h = size.Height.max(1) as u32;
        // --- Frame pool (free-threaded: no DispatcherQueue needed) ---
        let pool = windows::Graphics::Capture::Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device,
            windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| RecorderError::Capture(format!("frame pool: {e}")))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| RecorderError::Capture(format!("capture session: {e}")))?;
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|e| RecorderError::Capture(format!("StartCapture: {e}")))?;

        // --- Staging texture for CPU readback (native size) ---
        let staging: ID3D11Texture2D = {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: src_w,
                Height: src_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: Default::default(),
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: Default::default(),
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| RecorderError::Capture(format!("staging texture: {e}")))?;
            tex.unwrap()
        };

        // --- Resolution fit + swscale (bgra native -> nv12 target) ---
        let (dst_w, dst_h, scale) = fit_budget(src_w, src_h, budget);
        stats.width = dst_w;
        stats.height = dst_h;
        stats.native_width = src_w;
        stats.native_height = src_h;
        stats.scale = scale;
        stats.client_offset = (
            (native_client_offset.0 as f64 * (dst_w as f64 / src_w as f64)).round() as i32,
            (native_client_offset.1 as f64 * scale).round() as i32,
        );
        let mut sws = ffmpeg::software::scaling::Context::get(
            ffmpeg::format::Pixel::BGRA,
            src_w,
            src_h,
            ffmpeg::format::Pixel::NV12,
            dst_w,
            dst_h,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )
        .map_err(|e| RecorderError::Capture(format!("swscale: {e}")))?;
        let mut src_frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, src_w, src_h);
        let mut dst_frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::NV12, dst_w, dst_h);

        // --- In-process encoder; backend picked at the OUTPUT size ---
        let (encoder_name, encoder_opts) = video_encoder::pick_encoder_backend(
            explicit_encoder.as_deref(),
            dst_w,
            dst_h,
            fps,
        )?;
        stats.encoder = encoder_name.clone();
        let mut encoder =
            VideoEncoder::open(&out, &encoder_name, dst_w, dst_h, fps, &encoder_opts)
                .map_err(|e| RecorderError::Capture(format!("encoder open: {e}")))?;

        // --- Ready; wait for the session gate ---
        let _ = ready_tx.send("video");
        let clock = clock_rx
            .recv()
            .map_err(|e| RecorderError::Capture(format!("session gate closed: {e}")))?;

        // --- Encoder feeder: owns the in-process encoder ---
        let (frame_tx, frame_rx) = sync_channel::<Frame>(4);
        let feeder = std::thread::Builder::new()
            .name(format!("encode-{}", client.pid))
            .spawn(move || {
                let mut written = 0u64;
                let mut error: Option<String> = None;
                while let Ok(frame) = frame_rx.recv() {
                    if let Err(e) = encoder.encode(&frame.nv12) {
                        error = Some(e.to_string());
                        break;
                    }
                    written += 1;
                }
                let finish = encoder.finish();
                if error.is_none() {
                    if let Err(e) = finish {
                        error = Some(format!("finish: {e}"));
                    }
                }
                (written, error)
            })
            .expect("spawn encoder feeder");

        // --- CFR loop ---
        let tick = Duration::from_nanos(1_000_000_000 / fps.max(1) as u64);
        let src_stride = (src_w as usize) * 4;
        let mut bgra: Vec<u8> = vec![0u8; src_stride * src_h as usize];
        let mut next_tick = Instant::now() + tick;
        let mut warned_drops = 0u64;
        loop {
            if stop.load(Ordering::Relaxed) {
                stats.stopped_reason = "stopped".into();
                break;
            }
            if !client.window.is_some_and(|w| w.is_valid()) {
                stats.stopped_reason = "process_exit".into();
                break;
            }
            // Drain to the newest pending frame (drops older ones).
            loop {
                match pool.TryGetNextFrame() {
                    Ok(frame) => {
                        let surface = frame.Surface().map_err(|e| {
                            RecorderError::Capture(format!("frame surface: {e}"))
                        })?;
                        let access: IDirect3DDxgiInterfaceAccess = surface
                            .cast()
                            .map_err(|e| RecorderError::Capture(format!("surface access: {e}")))?;
                        let texture: ID3D11Texture2D = access
                            .GetInterface()
                            .map_err(|e| RecorderError::Capture(format!("GetInterface: {e}")))?;
                        context.CopyResource(&staging, &texture);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context
                            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                            .map_err(|e| RecorderError::Capture(format!("map: {e}")))?;
                        let raw = std::slice::from_raw_parts(
                            mapped.pData as *const u8,
                            mapped.RowPitch as usize * src_h as usize,
                        );
                        for row in 0..src_h as usize {
                            let src_off = row * mapped.RowPitch as usize;
                            bgra[row * src_stride..(row + 1) * src_stride]
                                .copy_from_slice(&raw[src_off..src_off + src_stride]);
                        }
                        context.Unmap(&staging, 0);
                    }
                    Err(_) => break,
                }
            }
            let frame_t_ms = clock.now_rel_ms();
            // Copy into the swscale input frame, scale to NV12, stamp.
            {
                let y_stride = src_frame.stride(0);
                let plane = src_frame.data_mut(0);
                for row in 0..src_h as usize {
                    let src = &bgra[row * src_stride..(row + 1) * src_stride];
                    plane[row * y_stride..row * y_stride + src_stride].copy_from_slice(src);
                }
            }
            sws.run(&src_frame, &mut dst_frame)
                .map_err(|e| RecorderError::Capture(format!("swscale run: {e}")))?;
            {
                let stamp = crate::watermark::format_stamp(frame_t_ms);
                let (y_stride, uv_stride) = (dst_frame.stride(0), dst_frame.stride(1));
                let y_plane = dst_frame.data_mut(0);
                let bar = draw_stamp_nv12_y(
                    y_plane,
                    y_stride,
                    dst_w as usize,
                    dst_h as usize,
                    &stamp,
                    2,
                );
                let uv_plane = dst_frame.data_mut(1);
                draw_stamp_nv12_uv(uv_plane, uv_stride, bar);
            }
            // Compact NV12 payload for the feeder.
            let y_stride = dst_frame.stride(0);
            let uv_stride = dst_frame.stride(1);
            let chroma_rows = (dst_h as usize + 1) / 2;
            let mut nv12 = Vec::with_capacity(dst_w as usize * dst_h as usize * 3 / 2);
            {
                let y_plane = dst_frame.data(0);
                for row in 0..dst_h as usize {
                    nv12
                        .extend_from_slice(&y_plane[row * y_stride..row * y_stride + dst_w as usize]);
                }
            }
            {
                let uv_plane = dst_frame.data(1);
                for row in 0..chroma_rows {
                    nv12.extend_from_slice(
                        &uv_plane[row * uv_stride..row * uv_stride + dst_w as usize],
                    );
                }
            }
            match frame_tx.try_send(Frame { nv12 }) {
                Ok(()) => {
                    if stats.anchor_t_ms.is_none() {
                        stats.anchor_t_ms = Some(frame_t_ms);
                    }
                    stats.frames_written += 1;
                }
                Err(TrySendError::Full(_)) => {
                    stats.frames_dropped += 1;
                    if stats.frames_dropped - warned_drops >= DROP_WARN_INTERVAL {
                        tracing::warn!(
                            pid = client.pid,
                            encoder = %encoder_name,
                            dropped = stats.frames_dropped,
                            written = stats.frames_written,
                            "encoder backlog: frames dropped to keep the video timeline exact"
                        );
                        warned_drops = stats.frames_dropped;
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    stats.stopped_reason = "encoder_input_closed".into();
                    break;
                }
            }

            let now = Instant::now();
            if next_tick > now {
                std::thread::sleep(next_tick - now);
                next_tick += tick;
            } else {
                next_tick = now + tick;
            }
        }

        drop(frame_tx);
        let (feeder_written, feeder_error) = feeder.join().expect("encoder feeder panicked");
        stats.frames_written = feeder_written;
        stats.encoder_error = feeder_error;
        if let Some(error) = &stats.encoder_error {
            tracing::warn!(pid = client.pid, error = %error, "in-process encoder failed");
        }
        if stats.frames_dropped > 0 {
            tracing::warn!(
                pid = client.pid,
                dropped = stats.frames_dropped,
                written = stats.frames_written,
                "video dropped frames total (encoder slower than target fps)"
            );
        }
        drop(session);
        let _ = pool.Close();
    }
    Ok(stats)
}

/// Client-area origin relative to the window origin (native video pixel
/// space), so recorded `client_x/y` aligns with the mp4 despite the
/// title bar.
fn window_client_offset(hwnd_raw: isize) -> (i32, i32) {
    let Some(handle) = eve_memory::WindowHandle::from_raw(hwnd_raw) else { return (0, 0) };
    unsafe {
        let mut window_rect = RECT::default();
        if GetWindowRect(handle.0, &mut window_rect).is_err() {
            return (0, 0);
        }
        match handle.client_rect_screen() {
            Ok(client) => (client.x - window_rect.left, client.y - window_rect.top),
            Err(_) => (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_budget_preserves_aspect_and_budget() {
        // 2586x1703 (~4.40M px) into the 720p budget.
        let (w, h, scale) = fit_budget(2586, 1703, Some(BUDGET_720P));
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(w as u64 * h as u64 <= BUDGET_720P);
        let src_ar = 2586.0 / 1703.0;
        let out_ar = w as f64 / h as f64;
        assert!(
            (src_ar - out_ar).abs() / src_ar < 0.02,
            "aspect preserved: {src_ar:.4} vs {out_ar:.4}"
        );
        assert!((scale - h as f64 / 1703.0).abs() < 1e-9);
        // 1080p keeps more pixels, still inside budget.
        let (w2, h2, _) = fit_budget(2586, 1703, Some(BUDGET_1080P));
        assert!(w2 * h2 > w * h);
        assert!(w2 as u64 * h2 as u64 <= BUDGET_1080P);
        // Native: unchanged (evened).
        let (w3, h3, s3) = fit_budget(2586, 1703, None);
        assert_eq!((w3, h3), (2586, 1702));
        assert!((s3 - 1702.0 / 1703.0).abs() < 1e-9);
        // Small source: never upscaled.
        let (w4, h4, _) = fit_budget(800, 600, Some(BUDGET_1080P));
        assert_eq!((w4, h4), (800, 600));
    }
}
