//! Memory access abstraction: one trait, two implementations.
//!
//! [`MemorySource`] is the seam between the live game process
//! ([`LiveProcess`], `ReadProcessMemory`) and an offline dump
//! ([`crate::dump::DumpSource`], replayed from a zip). Everything above this
//! module — CPython decoding, UI tree walking, semantic extraction — is
//! written against the trait, so the entire stack can be developed and
//! regression-tested offline against recorded samples.

use std::fmt;

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

/// Owned process handle with query + read access, closed on drop.
struct ProcessHandle(HANDLE);

// Process handles are safe to move across threads and to read through
// concurrently: `ReadProcessMemory`/`VirtualQueryEx` take no mutable state.
unsafe impl Send for ProcessHandle {}
unsafe impl Sync for ProcessHandle {}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A live game process opened for memory reading.
pub struct LiveProcess {
    handle: ProcessHandle,
    pub pid: u32,
}

impl LiveProcess {
    /// Open the process with query + vm-read rights (same user, no admin).
    pub fn open(pid: u32) -> Result<LiveProcess> {
        let handle = unsafe {
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
        }
        .map_err(|e| Error::OpenProcessFailed {
            pid,
            message: e.to_string(),
        })?;
        Ok(LiveProcess {
            handle: ProcessHandle(handle),
            pid,
        })
    }

    /// Enumerate committed regions via `VirtualQueryEx`, keeping readable
    /// ones (committed, not guard, not no-access).
    pub fn committed_regions(&self) -> Result<Vec<MemoryRegion>> {
        let mut regions = Vec::new();
        let mut address: u64 = 0;
        loop {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let bytes = unsafe {
                VirtualQueryEx(
                    self.handle.0,
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
        let mut received = 0usize;
        let result = unsafe {
            ReadProcessMemory(
                self.handle.0,
                address.0 as *const _,
                buffer.as_mut_ptr() as *mut _,
                buffer.len(),
                Some(&mut received),
            )
        };
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

impl fmt::Debug for LiveProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveProcess").field("pid", &self.pid).finish()
    }
}
