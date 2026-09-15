//! Memory access abstraction: one trait, two implementations.
//!
//! [`MemorySource`] is the seam between the live game process
//! ([`LiveProcess`], `ReadProcessMemory`) and an offline dump
//! ([`crate::dump::DumpSource`], replayed from a zip). Everything above this
//! module — CPython decoding, UI tree walking, semantic extraction — is
//! written against the trait, so the entire stack can be developed and
//! regression-tested offline against recorded samples.
//!
//! [`LiveProcess`] uses an EPHEMERAL handle by default: the
//! `PROCESS_VM_READ` handle exists only while reads are actually
//! flowing (a frame walk is a ~10-30 ms burst) and a small reaper
//! thread closes it within ~100 ms of idle. A polling self-check that
//! enumerates foreign handles therefore sees nothing while we idle —
//! the one signature such a check can reliably observe.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQueryEx,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

use crate::address::Address;
use crate::error::{Error, Result};

/// A committed, readable memory region of the target process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryRegion {
    pub base: Address,
    pub size_bytes: u64,
}

impl MemoryRegion {
    pub fn contains(&self, address: Address) -> bool {
        address >= self.base && address - self.base < self.size_bytes
    }

    pub fn end(&self) -> Address {
        self.base + self.size_bytes
    }
}

/// Uniform read access to a process address space, live or replayed.
pub trait MemorySource: Send + Sync {
    /// Read up to `buffer.len()` bytes at `address`, returning the number of
    /// bytes actually read. A fully unreadable page yields an error.
    fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize>;

    /// Read exactly `buffer.len()` bytes or fail.
    fn read_exact(&self, address: Address, buffer: &mut [u8]) -> Result<()> {
        let received = self.read(address, buffer)?;
        if received == buffer.len() {
            Ok(())
        } else {
            Err(Error::ShortRead {
                address: address.0,
                requested: buffer.len(),
                received,
            })
        }
    }

    /// Read a little-endian `u64` at `address`.
    fn read_u64(&self, address: Address) -> Result<u64> {
        let mut bytes = [0u8; 8];
        self.read_exact(address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    /// Read a little-endian `f64` at `address`.
    fn read_f64(&self, address: Address) -> Result<f64> {
        let mut bytes = [0u8; 8];
        self.read_exact(address, &mut bytes)?;
        Ok(f64::from_le_bytes(bytes))
    }

    /// Enumerate committed readable regions, ordered by base address.
    fn regions(&self) -> Result<Vec<MemoryRegion>>;

    /// Human-readable description for logs and error messages.
    fn describe(&self) -> String;
}

/// How long the handle stays open after the last read before the
/// reaper closes it. A frame-walk burst (~10-30 ms) always fits inside
/// one handle; the gaps between frames release it.
const HANDLE_IDLE_CLOSE_MS: u64 = 100;

/// Shared ephemeral-handle state between the reader and its reaper.
#[derive(Clone)]
struct SharedHandle {
    inner: Arc<Mutex<Option<HANDLE>>>,
    pid: u32,
    /// Milliseconds since the reader's birth of the LAST read — the
    /// reaper closes the handle once `now - last_read` exceeds the idle
    /// threshold.
    last_read_ms: Arc<AtomicU64>,
    born: Arc<Instant>,
}

impl SharedHandle {
    fn open_raw(pid: u32) -> Result<HANDLE> {
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
            .map_err(|e| Error::OpenProcessFailed {
                pid,
                message: e.to_string(),
            })
    }

    fn now_ms(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    /// The handle, (re)opened if needed. Bursts reuse the open handle;
    /// after an idle gap the reaper has closed it and this reopens.
    fn get(&self) -> Result<HANDLE> {
        let mut guard = self.inner.lock().expect("handle lock");
        if guard.is_none() {
            *guard = Some(Self::open_raw(self.pid)?);
        }
        Ok(guard.unwrap())
    }

    fn note_read(&self) {
        self.last_read_ms
            .store(self.now_ms(), Ordering::Relaxed);
    }

    fn close_if_idle(&self) {
        let last = self.last_read_ms.load(Ordering::Relaxed);
        if self.now_ms().saturating_sub(last) < HANDLE_IDLE_CLOSE_MS {
            return;
        }
        if let Some(handle) = self.inner.lock().expect("handle lock").take() {
            let _ = unsafe { CloseHandle(handle) };
        }
    }

    /// For diagnostics: is the handle currently open?
    fn is_open(&self) -> bool {
        self.inner.lock().expect("handle lock").is_some()
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.inner.lock().expect("handle lock").take() {
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

// Process handles are safe to read through concurrently:
// ReadProcessMemory/VirtualQueryEx take no mutable state.
unsafe impl Send for SharedHandle {}
unsafe impl Sync for SharedHandle {}

/// A live game process opened for memory reading. The handle is
/// EPHEMERAL by default: held only while reads flow, closed by a
/// reaper thread within ~100 ms of idle.
pub struct LiveProcess {
    handle: SharedHandle,
    pub pid: u32,
    shutdown: Arc<AtomicBool>,
    reaper: Option<std::thread::JoinHandle<()>>,
}

impl LiveProcess {
    /// Open the process with query + vm-read rights (same user, no
    /// admin). Validates access up front; the read handle itself is
    /// opened lazily on first read and released while idle.
    pub fn open(pid: u32) -> Result<LiveProcess> {
        // Fail fast on wrong pid / access denied (and release the probe).
        let probe = SharedHandle::open_raw(pid)?;
        let _ = unsafe { CloseHandle(probe) };

        let handle = SharedHandle {
            inner: Arc::new(Mutex::new(None)),
            pid,
            last_read_ms: Arc::new(AtomicU64::new(0)),
            born: Arc::new(Instant::now()),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let reaper_handle = handle.clone();
        let flag = Arc::clone(&shutdown);
        // Start the idle clock at "never read": first reaper tick would
        // close an unopened handle harmlessly; a read burst resets it.
        reaper_handle.last_read_ms.store(0, Ordering::Relaxed);
        let reaper = std::thread::Builder::new()
            .name(format!("handle-reaper-{pid}"))
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(20));
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    reaper_handle.close_if_idle();
                }
            })
            .expect("spawn handle reaper");
        Ok(LiveProcess {
            handle,
            pid,
            shutdown,
            reaper: Some(reaper),
        })
    }

    /// Diagnostics: whether the read handle is currently open.
    pub fn handle_is_open(&self) -> bool {
        self.handle.is_open()
    }

    /// Enumerate committed regions via `VirtualQueryEx`, keeping readable
    /// ones (committed, not guard, not no-access).
    pub fn committed_regions(&self) -> Result<Vec<MemoryRegion>> {
        let handle = self.handle.get()?;
        self.handle.note_read();
        let mut regions = Vec::new();
        let mut address: u64 = 0;
        loop {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let bytes = unsafe {
                VirtualQueryEx(
                    handle,
                    Some(address as *const _),
                    &mut info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if bytes == 0 {
                break;
            }
            let base = info.BaseAddress as u64;
            let size = info.RegionSize as u64;
            if size == 0 {
                break;
            }
            let committed = info.State == MEM_COMMIT;
            let readable = (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0;
            if committed && readable {
                regions.push(MemoryRegion {
                    base: Address(base),
                    size_bytes: size,
                });
            }
            match address.checked_add(size) {
                Some(next) if next > address => address = next,
                _ => break,
            }
        }
        if regions.is_empty() {
            return Err(Error::RegionEnumerationFailed {
                pid: self.pid,
                message: "no committed regions found".into(),
            });
        }
        Ok(regions)
    }
}

impl MemorySource for LiveProcess {
    fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let handle = self.handle.get()?;
        let mut received = 0usize;
        let result = unsafe {
            ReadProcessMemory(
                handle,
                address.0 as *const _,
                buffer.as_mut_ptr() as *mut _,
                buffer.len(),
                Some(&mut received),
            )
        };
        self.handle.note_read();
        match result {
            Ok(()) => Ok(received),
            Err(e) => Err(Error::read_failed(address.0, e.to_string())),
        }
    }

    fn regions(&self) -> Result<Vec<MemoryRegion>> {
        self.committed_regions()
    }

    fn describe(&self) -> String {
        format!("live process {}", self.pid)
    }
}

impl Drop for LiveProcess {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(reaper) = self.reaper.take() {
            let _ = reaper.join();
        }
    }
}

impl fmt::Debug for LiveProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveProcess").field("pid", &self.pid).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ephemeral handle: open while reads flow, closed by the
    /// reaper within ~100 ms of idle, reopened on the next read.
    #[test]
    fn ephemeral_handle_closes_when_idle_and_reopens() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping", "-n", "20", "127.0.0.1"])
            .spawn()
            .expect("spawn sacrificial cmd");
        let source = LiveProcess::open(child.id()).expect("open child");

        // Drive a read burst: enumerate + one actual read.
        let regions = source.committed_regions().expect("regions");
        let mut probe = [0u8; 8];
        MemorySource::read(&source, regions[0].base, &mut probe).expect("read");
        assert!(source.handle_is_open(), "handle must exist during a burst");

        // Idle past the close threshold → the reaper released it.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !source.handle_is_open(),
            "handle must be closed after idle"
        );

        // The next burst transparently reopens it.
        let mut probe = [0u8; 8];
        MemorySource::read(&source, regions[0].base, &mut probe).expect("reopen read");
        assert!(source.handle_is_open(), "handle must reopen for a new burst");

        let _ = child.kill();
        let _ = child.wait();
    }
}
