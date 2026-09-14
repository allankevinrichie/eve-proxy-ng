//! Offline dumps: save a process's committed memory to a zip and replay it.
//!
//! The archive layout matches the Sanderling `ProcessSample` format so
//! community samples can be replayed too:
//!
//! ```text
//! Process/Memory/0x<region-base-hex>   one raw file per committed region
//! copy-memory-log                       region enumeration log (UTF-8)
//! begin-main-window-client-area.bmp     screenshot before the copy
//! end-main-window-client-area.bmp       screenshot after the copy
//! ```
//!
//! Replay needs no game client: a [`DumpSource`] serves the recorded bytes
//! through the [`MemorySource`] trait, so the whole stack above it —
//! decoding, tree walking, semantics — runs offline and reproducibly.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::address::Address;
use crate::error::{Error, Result};
use crate::source::{LiveProcess, MemoryRegion, MemorySource};
use crate::window::WindowHandle;

/// Record a full dump of `live` to `directory`.
///
/// Returns the written archive path
/// (`process-sample-<sha256[..10]>.zip`, named like the reference tool).
/// If the client window is given and visible, before/after client-area
/// screenshots are embedded.
pub fn save_process_sample(
    live: &LiveProcess,
    window: Option<WindowHandle>,
    delay: Option<std::time::Duration>,
    directory: &Path,
) -> Result<PathBuf> {
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
    fs::create_dir_all(directory)?;
    let temp_path = directory.join("process-sample-partial.zip");

    let begin_screenshot = capture(window);
    let regions = live.committed_regions()?;
    tracing::info!(count = regions.len(), "copying committed regions");

    let mut log_lines: Vec<String> = Vec::with_capacity(regions.len());
    let mut copied: Vec<(MemoryRegion, Vec<u8>)> = Vec::with_capacity(regions.len());
    for region in &regions {
        let mut buffer = vec![0u8; region.size_bytes as usize];
        match live.read_exact(region.base, &mut buffer) {
            Ok(()) => {
                log_lines.push(format!(
                    "0x{:x} {} bytes",
                    region.base.0, region.size_bytes
                ));
                copied.push((*region, buffer));
            }
            Err(e) => {
                tracing::warn!(region = %region.base, error = %e, "skipping unreadable region");
                log_lines.push(format!(
                    "0x{:x} {} bytes SKIPPED: {e}",
                    region.base.0, region.size_bytes
                ));
            }
        }
    }
    let end_screenshot = capture(window);

    let file = fs::File::create(&temp_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (region, bytes) in &copied {
        writer.start_file(format!("Process/Memory/0x{:X}", region.base.0), options)?;
        writer.write_all(bytes)?;
    }
    writer.start_file("copy-memory-log", options)?;
    writer.write_all(log_lines.join("\n").as_bytes())?;
    for (name, image) in [
        ("begin-main-window-client-area.bmp", begin_screenshot),
        ("end-main-window-client-area.bmp", end_screenshot),
    ] {
        if let Some(image) = image {
            writer.start_file(name, options)?;
            let mut bytes = Vec::new();
            image
                .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Bmp)
                .map_err(|e| Error::CaptureFailed {
                    message: format!("bmp encode: {e}"),
                })?;
            writer.write_all(&bytes)?;
        }
    }
    writer.finish()?;
    drop(copied);

    let bytes = fs::read(&temp_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = format!("{:x}", hasher.finalize());
    let final_path = directory.join(format!("process-sample-{}.zip", &digest[..10]));
    fs::rename(&temp_path, &final_path)?;
    tracing::info!(path = %final_path.display(), bytes = bytes.len(), "sample saved");
    Ok(final_path)
}

fn capture(window: Option<WindowHandle>) -> Option<image::RgbImage> {
    let window = window?;
    if window.is_minimized() || !window.is_visible() {
        tracing::debug!("window not capturable, skipping screenshot");
        return None;
    }
    match window.capture_client_area() {
        Ok(image) => Some(image),
        Err(e) => {
            tracing::warn!(error = %e, "screenshot failed");
            None
        }
    }
}

/// A loaded dump, serving recorded regions as a [`MemorySource`].
pub struct DumpSource {
    regions: Vec<DumpRegion>,
    pub copy_memory_log: Option<String>,
}

struct DumpRegion {
    base: Address,
    data: Vec<u8>,
}

impl DumpSource {
    /// Load a sample archive written by this tool or by Sanderling's
    /// `save-process-sample`.
    pub fn open_zip(path: &Path) -> Result<DumpSource> {
        let file = fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| Error::MalformedSample {
                message: format!("open zip: {e}"),
            })?;
        let mut regions: Vec<DumpRegion> = Vec::new();
        let mut copy_memory_log = None;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| Error::MalformedSample {
                    message: format!("entry {index}: {e}"),
                })?;
            let name = entry.name().replace('\\', "/");
            if name == "copy-memory-log" {
                let mut text = String::new();
                entry
                    .read_to_string(&mut text)
                    .map_err(|e| Error::MalformedSample {
                        message: format!("copy-memory-log: {e}"),
                    })?;
                copy_memory_log = Some(text);
                continue;
            }
            let Some(rest) = name.strip_prefix("Process/Memory/") else {
                continue;
            };
            // Single path component only; nested entries are not regions.
            if rest.contains('/') {
                continue;
            }
            let Some(hex) = rest.strip_prefix("0x") else {
                continue;
            };
            let base = u64::from_str_radix(hex, 16).map_err(|e| Error::MalformedSample {
                message: format!("region name {rest:?}: {e}"),
            })?;
            let mut data = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut data)
                .map_err(|e| Error::MalformedSample {
                    message: format!("region {rest}: {e}"),
                })?;
            regions.push(DumpRegion {
                base: Address(base),
                data,
            });
        }
        if regions.is_empty() {
            return Err(Error::MalformedSample {
                message: "no Process/Memory/0x… entries found".into(),
            });
        }
        regions.sort_by_key(|r| r.base);
        // Overlapping regions would make reads ambiguous; the recorder never
        // produces them, but tolerate overlapping community samples by
        // keeping the first (lower) region.
        regions.dedup_by(|a, b| a.base == b.base);
        tracing::info!(
            regions = regions.len(),
            bytes = regions.iter().map(|r| r.data.len()).sum::<usize>(),
            "sample loaded"
        );
        Ok(DumpSource {
            regions,
            copy_memory_log,
        })
    }

    fn region_containing(&self, address: Address) -> Option<&DumpRegion> {
        let index = self
            .regions
            .partition_point(|r| r.base <= address)
            .checked_sub(1)?;
        let region = &self.regions[index];
        let offset = address - region.base;
        if offset < region.data.len() as u64 {
            Some(region)
        } else {
            None
        }
    }
}

impl MemorySource for DumpSource {
    fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let Some(region) = self.region_containing(address) else {
            return Err(Error::ReadFailed {
                address: address.0,
                message: "not covered by dump".into(),
            });
        };
        let offset = (address - region.base) as usize;
        let available = region.data.len() - offset;
        let count = buffer.len().min(available);
        buffer[..count].copy_from_slice(&region.data[offset..offset + count]);
        Ok(count)
    }

    fn regions(&self) -> Result<Vec<MemoryRegion>> {
        Ok(self
            .regions
            .iter()
            .map(|r| MemoryRegion {
                base: r.base,
                size_bytes: r.data.len() as u64,
            })
            .collect())
    }

    fn describe(&self) -> String {
        "dump replay".into()
    }
}
