//! Block-caching decorator for [`MemorySource`]: one underlying read per
//! 4 KiB page-aligned block, all smaller reads served from the local
//! copy.
//!
//! ~90% of tree-walk reads are ≤100-byte header fetches scattered over a
//! heap whose objects cluster into a few thousand pages, so a few
//! thousand page reads replace a hundred thousand kernel transitions
//! while transferring only the pages the tree actually touches (a 64 KiB
//! granularity would fetch ~68x more bytes than the tree occupies).
//!
//! Correctness model: a `CachedSource` instance lives for exactly ONE
//! tree read (the `UiReader` creates a fresh one per `read_tree_at`
//! call). Within that read the cache is a point-in-time snapshot — every
//! block it holds was fetched during this frame — and the next frame
//! starts from an empty cache, so dynamic values (text, coords, state)
//! are always re-read. Layout-agnostic by construction: it sits below
//! `PythonLayout` and will survive the client's Python 3 migration
//! unchanged.

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::address::Address;
use crate::error::{Error, Result};
use crate::source::MemorySource;

const BLOCK_BITS: u32 = 12; // 4 KiB, page-aligned
const BLOCK_SIZE: u64 = 1 << BLOCK_BITS;
const BLOCK_MASK: u64 = BLOCK_SIZE - 1;

struct Block {
    /// Fetched bytes; shorter than BLOCK_SIZE when the underlying read
    /// stopped early (end of a committed region).
    data: Vec<u8>,
}

/// One-read-per-block view over `inner`. Meant for a single walking
/// thread; `Sync` only to satisfy the [`MemorySource`] supertrait.
pub struct CachedSource<'a> {
    inner: &'a dyn MemorySource,
    blocks: Mutex<FxHashMap<u64, Arc<Block>>>,
    /// Diagnostics: underlying reads issued vs calls served.
    reads_served: AtomicU64,
    blocks_fetched: AtomicU64,
}

impl<'a> CachedSource<'a> {
    pub fn new(inner: &'a dyn MemorySource) -> Self {
        Self {
            inner,
            blocks: Mutex::new(FxHashMap::default()),
            reads_served: AtomicU64::new(0),
            blocks_fetched: AtomicU64::new(0),
        }
    }

    /// Cache-fill statistics for the completed read (see the `bench`
    /// CLI): how many underlying reads the block cache saved.
    pub fn stats(&self) -> (u64, u64) {
        (self.reads_served.load(Ordering::Relaxed), self.blocks_fetched.load(Ordering::Relaxed))
    }

    fn fetch_block(&self, base: u64) -> Result<Arc<Block>> {
        if let Some(block) = self.blocks.lock().unwrap().get(&base) {
            return Ok(block.clone());
        }
        let mut data = vec![0u8; BLOCK_SIZE as usize];
        let fetched = self.inner.read(Address(base), &mut data)?;
        if fetched == 0 {
            return Err(Error::ReadFailed {
                address: base,
                message: "block fetch returned nothing".into(),
            });
        }
        data.truncate(fetched);
        let block = Arc::new(Block { data });
        self.blocks.lock().unwrap().insert(base, block.clone());
        self.blocks_fetched.fetch_add(1, Ordering::Relaxed);
        Ok(block)
    }
}

impl MemorySource for CachedSource<'_> {
    fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.reads_served.fetch_add(1, Ordering::Relaxed);
        let mut filled = 0usize;
        while filled < buffer.len() {
            let addr = address.0 + filled as u64;
            let base = addr & !BLOCK_MASK;
            let offset = (addr - base) as usize;
            match self.fetch_block(base) {
                Ok(block) => {
                    // Truncated tail blocks may end before this segment.
                    let take = block
                        .data
                        .len()
                        .checked_sub(offset)
                        .map(|avail| avail.min(buffer.len() - filled))
                        .unwrap_or(0);
                    if take == 0 {
                        // The block ended (region boundary) before this
                        // segment: forward directly so partial-read
                        // semantics stay identical to the inner source.
                        let n = self.inner.read(Address(addr), &mut buffer[filled..])?;
                        return Ok(filled + n);
                    }
                    buffer[filled..filled + take]
                        .copy_from_slice(&block.data[offset..offset + take]);
                    filled += take;
                }
                Err(_) => {
                    // Whole-block fetch failed (uncommitted page inside
                    // it): serve this segment directly so the walker sees
                    // the same partial-read/error semantics as before.
                    let n = self.inner.read(Address(addr), &mut buffer[filled..])?;
                    return Ok(filled + n);
                }
            }
        }
        Ok(filled)
    }

    fn regions(&self) -> Result<Vec<crate::source::MemoryRegion>> {
        self.inner.regions()
    }

    fn describe(&self) -> String {
        format!("{} + 4KiB page cache", self.inner.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryRegion;

    /// Byte-addressable fake source for exactness checks.
    struct Linear {
        bytes: Vec<u8>,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl Linear {
        fn new(size: u64) -> Self {
            Self {
                bytes: (0..size).map(|i| (i % 251) as u8).collect(),
                reads: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl MemorySource for Linear {
        fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let start = address.0 as usize;
            if start >= self.bytes.len() {
                return Ok(0);
            }
            let take = buffer.len().min(self.bytes.len() - start);
            buffer[..take].copy_from_slice(&self.bytes[start..start + take]);
            Ok(take)
        }

        fn regions(&self) -> Result<Vec<MemoryRegion>> {
            Ok(vec![MemoryRegion { base: Address(0), size_bytes: self.bytes.len() as u64 }])
        }

        fn describe(&self) -> String {
            "linear".into()
        }
    }

    #[test]
    fn serves_exact_bytes_and_coalesces_reads() {
        let inner = Linear::new(BLOCK_SIZE * 3);
        let cached = CachedSource::new(&inner);
        // Many scattered small reads across two blocks.
        for offset in (0..200u64).step_by(7) {
            let mut buf = [0u8; 13];
            let n = cached.read(Address(BLOCK_SIZE + offset), &mut buf).unwrap();
            assert_eq!(n, 13);
            let want: Vec<u8> =
                ((BLOCK_SIZE + offset)..(BLOCK_SIZE + offset + 13)).map(|i| (i % 251) as u8).collect();
            assert_eq!(&buf[..], &want[..]);
        }
        let (served, fetched) = cached.stats();
        assert_eq!(served, 29, "one read per step_by(7) call over 200");
        assert_eq!(fetched, 1, "all reads landed in one 64KiB block");
        assert_eq!(inner.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn read_spanning_blocks_splits() {
        let inner = Linear::new(BLOCK_SIZE * 2);
        let cached = CachedSource::new(&inner);
        let start = BLOCK_SIZE - 8;
        let mut buf = vec![0u8; 20];
        let n = cached.read(Address(start), &mut buf).unwrap();
        assert_eq!(n, 20);
        let want: Vec<u8> = (start..start + 20).map(|i| (i % 251) as u8).collect();
        assert_eq!(buf, want);
        assert_eq!(cached.stats().1, 2, "crossing a boundary fetched both blocks");
    }

    #[test]
    fn end_of_region_partial_read() {
        let inner = Linear::new(BLOCK_SIZE + 100);
        let cached = CachedSource::new(&inner);
        let mut buf = vec![0u8; 16];
        // Read straddling the end of data: first 10 bytes exist, rest fail.
        let start = BLOCK_SIZE + 90;
        let n = cached.read(Address(start), &mut buf).unwrap();
        assert_eq!(n, 10);
        let want: Vec<u8> = (start..start + 10).map(|i| (i % 251) as u8).collect();
        assert_eq!(&buf[..10], &want[..]);
    }

    #[test]
    fn failed_block_fetch_falls_back() {
        // A source whose first block is unreadable but small reads work.
        struct Spotty;
        impl MemorySource for Spotty {
            fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
                if buffer.len() > 1024 {
                    return Err(Error::ReadFailed { address: address.0, message: "large read rejected".into() });
                }
                for (i, b) in buffer.iter_mut().enumerate() {
                    *b = (address.0 as usize + i) as u8;
                }
                Ok(buffer.len())
            }
            fn regions(&self) -> Result<Vec<MemoryRegion>> {
                Ok(vec![MemoryRegion { base: Address(0), size_bytes: BLOCK_SIZE }])
            }
            fn describe(&self) -> String {
                "spotty".into()
            }
        }
        let spotty = Spotty;
        let cached = CachedSource::new(&spotty);
        let mut buf = [0u8; 32];
        let n = cached.read(Address(5), &mut buf).unwrap();
        assert_eq!(n, 32);
        assert_eq!(buf[0], 5);
    }
}
