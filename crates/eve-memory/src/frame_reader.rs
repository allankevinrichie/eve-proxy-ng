//! Frame cache with overlapped prefetch and parallel walking: one
//! underlying read per 4 KiB page, all smaller reads served from the
//! local copy.
//!
//! The page table is shared between any number of threads — the
//! prefetch thread (racing fill of the previous frame's page set) and
//! the rayon workers of a parallel tree walk — via a sharded mutex table
//! with `Arc` page handles. When [`FrameReader::set_parallel`] marks
//! multi-threaded walking, the single-thread `last` register fast path
//! is disabled (it would be a data race) and every read goes through the
//! shard table, whose per-address sharding keeps lock contention low.
//!
//! Correctness model (unchanged): an instance lives for exactly ONE
//! tree read. Every page it holds was fetched during this frame, and the
//! next frame starts empty — dynamic values (text, coords, state) are
//! always re-read. A page read twice within a frame (walker miss racing
//! the prefetch, or two walkers) yields same-frame data, never a stale
//! one. Layout-agnostic by construction; survives the client's Python 3
//! migration unchanged.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;

use crate::address::Address;
use crate::error::{Error, Result};
use crate::source::MemorySource;

const PAGE_BITS: u32 = 12; // 4 KiB
const PAGE_SIZE: u64 = 1 << PAGE_BITS;
const PAGE_MASK: u64 = PAGE_SIZE - 1;

const SHARD_BITS: u32 = 4; // 16 shards
const SHARDS: usize = 1 << SHARD_BITS;

#[derive(Default)]
struct ShardTable {
    shards: [Mutex<FxHashMap<u64, Arc<[u8]>>>; SHARDS],
}

impl ShardTable {
    #[inline]
    fn shard_index(base: u64) -> usize {
        // Fold the address's upper bits (the page number already covers
        // the low 12) so adjacent pages land in different shards.
        ((base >> PAGE_BITS) as usize) & (SHARDS - 1)
    }

    fn get(&self, base: u64) -> Option<Arc<[u8]>> {
        self.shards[Self::shard_index(base)].lock().unwrap().get(&base).cloned()
    }

    fn insert(&self, base: u64, page: Arc<[u8]>) {
        self.shards[Self::shard_index(base)].lock().unwrap().insert(base, page);
    }
}

/// Per-frame page cache over an inner source.
pub struct FrameReader<'a> {
    inner: &'a dyn MemorySource,
    pages: ShardTable,
    /// Pages actually READ this frame (prefetch loads don't count) —
    /// the basis for the next frame's prefetch set, so the set tracks
    /// what the walk really uses and shrinks when the UI does.
    used: Mutex<rustc_hash::FxHashSet<u64>>,
    /// Walking-thread-exclusive hot register, used only while walking is
    /// single-threaded (see [`FrameReader::set_parallel`]).
    last: RefCell<Option<(u64, Arc<[u8]>)>>,
    parallel: AtomicBool,
    owner_thread: Cell<std::thread::ThreadId>,
    /// Diagnostics (touched by all threads).
    reads_served: AtomicU64,
    pages_fetched: AtomicU64,
    last_page_hits: AtomicU64,
    prefetched_pages: AtomicU64,
}

// SAFETY: the page table and used set are mutex-guarded with `Arc`
// handles; the `last` register and `owner_thread` are only touched by
// the constructing thread and only while `parallel` is false (which
// `read` enforces on the fast path and debug builds assert).
unsafe impl Send for FrameReader<'_> {}
unsafe impl Sync for FrameReader<'_> {}

impl<'a> FrameReader<'a> {
    pub fn new(inner: &'a dyn MemorySource) -> Self {
        Self {
            inner,
            pages: ShardTable::default(),
            last: RefCell::new(None),
            used: Mutex::new(rustc_hash::FxHashSet::default()),
            parallel: AtomicBool::new(false),
            owner_thread: Cell::new(std::thread::current().id()),
            reads_served: AtomicU64::new(0),
            pages_fetched: AtomicU64::new(0),
            last_page_hits: AtomicU64::new(0),
            prefetched_pages: AtomicU64::new(0),
        }
    }

    /// Mark the frame as walked by multiple threads: disables the
    /// single-thread `last`-register fast path (it would be a data
    /// race); reads then always go through the sharded page table.
    pub fn set_parallel(&self) {
        self.parallel.store(true, Ordering::Release);
    }

    /// Diagnostics for the completed frame: `(reads served, pages
    /// fetched, last-page hits, prefetched pages)`.
    pub fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.reads_served.load(Ordering::Relaxed),
            self.pages_fetched.load(Ordering::Relaxed),
            self.last_page_hits.load(Ordering::Relaxed),
            self.prefetched_pages.load(Ordering::Relaxed),
        )
    }

    /// Page base addresses actually read this frame.
    pub fn touched_pages(&self) -> Vec<u64> {
        self.used.lock().unwrap().iter().copied().collect()
    }

    fn fetch_page(&self, base: u64) -> Result<Arc<[u8]>> {
        self.used.lock().unwrap().insert(base);
        if let Some(page) = self.pages.get(base) {
            return Ok(page);
        }
        let mut data = vec![0u8; PAGE_SIZE as usize];
        let fetched = self.inner.read(Address(base), &mut data)?;
        if fetched == 0 {
            return Err(Error::ReadFailed {
                address: base,
                message: "page fetch returned nothing".into(),
            });
        }
        data.truncate(fetched);
        let page: Arc<[u8]> = Arc::from(data);
        self.pages.insert(base, page.clone());
        self.pages_fetched.fetch_add(1, Ordering::Relaxed);
        Ok(page)
    }

    /// Bulk-prefetch: fetch every page in `bases` (sorted, grouped into
    /// contiguous runs with gaps below `max_gap`, each run capped at
    /// MAX_RUN) via large sequential reads from the inner source. Safe to
    /// call from any thread concurrently with walking (racing fill).
    /// Pages that fail (decommitted etc.) are simply left uncached —
    /// later reads fall back to single-page fetches.
    ///
    /// The run cap matters: a dense page cluster chains into runs, and
    /// without a cap one chain can swallow the whole heap (observed
    /// ~19 GiB in one read).
    pub fn prefetch(&self, bases: &[u64], max_gap: u64) -> usize {
        const MAX_RUN: u64 = 1024 * 1024;
        let mut sorted = bases.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        // Disjoint, sorted, gap-merged runs, each capped at MAX_RUN.
        let mut runs: Vec<(u64, u64)> = Vec::new(); // (start, end exclusive)
        for &base in &sorted {
            let page_end = base + PAGE_SIZE;
            let can_extend = match runs.last() {
                Some(&(start, end)) => {
                    base <= end.saturating_add(max_gap) && page_end - start <= MAX_RUN
                }
                None => false,
            };
            if can_extend {
                let (_, end) = runs.last_mut().unwrap();
                *end = (*end).max(page_end);
            } else {
                runs.push((base, page_end));
            }
        }
        let mut prefetched = 0usize;
        for (start, end) in runs {
            let len = (end - start) as usize;
            let mut data = vec![0u8; len];
            if self.inner.read(Address(start), &mut data).is_err() {
                continue; // fall back to per-page fetch on demand
            }
            let data: Arc<[u8]> = Arc::from(data);
            let mut run_pages = 0u64;
            let mut offset = 0u64;
            while offset < len as u64 {
                let base = start + offset;
                let take = PAGE_SIZE.min(len as u64 - offset) as usize;
                let slice: Arc<[u8]> = Arc::from(&data[offset as usize..offset as usize + take]);
                self.pages.insert(base, slice);
                run_pages += 1;
                offset += PAGE_SIZE;
            }
            prefetched += run_pages as usize;
            self.prefetched_pages.fetch_add(run_pages, Ordering::Relaxed);
            self.pages_fetched.fetch_add((len as u64).div_ceil(PAGE_SIZE), Ordering::Relaxed);
        }
        prefetched
    }
}

impl MemorySource for FrameReader<'_> {
    fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
        let single_threaded = !self.parallel.load(Ordering::Acquire);
        debug_assert!(
            !single_threaded || self.owner_thread.get() == std::thread::current().id(),
            "single-thread reads are walking-thread exclusive by contract"
        );
        if buffer.is_empty() {
            return Ok(0);
        }
        self.reads_served.fetch_add(1, Ordering::Relaxed);
        let start = address.0;
        let end = start + buffer.len() as u64;
        let first_base = start & !PAGE_MASK;
        // Fast path (single-threaded walks only): the whole read lands
        // inside the last page served.
        if single_threaded {
            let last = self.last.borrow();
            if let Some((base, page)) = last.as_ref() {
                let base = *base;
                if base == first_base && end - base <= page.len() as u64 {
                    let offset = (start - base) as usize;
                    buffer.copy_from_slice(&page[offset..offset + buffer.len()]);
                    self.last_page_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(buffer.len());
                }
            }
        }
        // General path: fill from the sharded page map, fetching as
        // needed. Prefetch and sibling walkers may fill pages
        // concurrently — an overwrite carries same-frame data.
        let mut filled = 0usize;
        while filled < buffer.len() {
            let addr = start + filled as u64;
            let base = addr & !PAGE_MASK;
            let offset = (addr - base) as usize;
            let page = self.fetch_page(base)?;
            if single_threaded {
                *self.last.borrow_mut() = Some((base, page.clone()));
            }
            let take = page.len().saturating_sub(offset).min(buffer.len() - filled);
            if take == 0 {
                // Page truncated at a region boundary: forward the rest
                // directly so partial-read semantics match the inner
                // source.
                let n = self.inner.read(Address(addr), &mut buffer[filled..])?;
                return Ok(filled + n);
            }
            buffer[filled..filled + take].copy_from_slice(&page[offset..offset + take]);
            filled += take;
        }
        Ok(filled)
    }

    fn regions(&self) -> Result<Vec<crate::source::MemoryRegion>> {
        self.inner.regions()
    }

    fn describe(&self) -> String {
        format!("{} + frame page cache", self.inner.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryRegion;

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

    fn want(start: u64, len: usize) -> Vec<u8> {
        (start..start + len as u64).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn serves_exact_bytes_with_last_page_fast_path() {
        let inner = Linear::new(PAGE_SIZE * 2);
        let reader = FrameReader::new(&inner);
        let mut buf = [0u8; 13];
        reader.read(Address(PAGE_SIZE + 64), &mut buf).unwrap();
        assert_eq!(&buf[..], &want(PAGE_SIZE + 64, 13)[..]);
        reader.read(Address(PAGE_SIZE + 96), &mut buf).unwrap();
        assert_eq!(&buf[..], &want(PAGE_SIZE + 96, 13)[..]);
        reader.read(Address(PAGE_SIZE + 300), &mut buf).unwrap();
        let (_, fetched, hits, _) = reader.stats();
        assert_eq!(fetched, 1, "all reads on one page");
        assert!(hits >= 2, "register served the repeats");
        assert_eq!(inner.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn read_spanning_pages_splits() {
        let inner = Linear::new(PAGE_SIZE * 2);
        let reader = FrameReader::new(&inner);
        let start = PAGE_SIZE - 8;
        let mut buf = vec![0u8; 20];
        let n = reader.read(Address(start), &mut buf).unwrap();
        assert_eq!(n, 20);
        assert_eq!(buf, want(start, 20));
        let (_, fetched, _, _) = reader.stats();
        assert_eq!(fetched, 2);
    }

    #[test]
    fn parallel_mode_multi_thread_reads() {
        let inner = Linear::new(PAGE_SIZE * 32);
        let reader = FrameReader::new(&inner);
        reader.set_parallel();
        std::thread::scope(|scope| {
            for t in 0..4u64 {
                let reader = &reader;
                scope.spawn(move || {
                    let mut buf = [0u8; 24];
                    for i in 0..64u64 {
                        let addr = ((i * 991 + t * PAGE_SIZE) % (PAGE_SIZE * 32))
                            .max(PAGE_SIZE * 4); // stay above page 4
                        let n = reader.read(Address(addr), &mut buf).unwrap();
                        assert_eq!(n, 24);
                        assert_eq!(&buf[..], &want(addr, 24)[..]);
                    }
                });
            }
        });
        let (served, _, hits, _) = reader.stats();
        assert_eq!(served, 4 * 64);
        assert_eq!(hits, 0, "no register in parallel mode");
    }

    #[test]
    fn concurrent_prefetch_and_walk_race_safely() {
        let inner = Linear::new(PAGE_SIZE * 16);
        let reader = FrameReader::new(&inner);
        let bases: Vec<u64> = (0..16).map(|i| i * PAGE_SIZE).collect();
        std::thread::scope(|scope| {
            let prefetch = scope.spawn(|| reader.prefetch(&bases, PAGE_SIZE));
            let mut buf = [0u8; 24];
            for i in 0..64u64 {
                let addr = (i * 997) % (PAGE_SIZE * 16);
                let n = reader.read(Address(addr), &mut buf).unwrap();
                assert_eq!(n, 24);
                assert_eq!(&buf[..], &want(addr, 24)[..]);
            }
            prefetch.join().unwrap();
        });
        let touched = reader.touched_pages();
        assert!(touched.len() <= 16);
    }

    #[test]
    fn prefetch_groups_gaps_and_serves() {
        let inner = Linear::new(PAGE_SIZE * 10);
        let reader = FrameReader::new(&inner);
        let bases = vec![PAGE_SIZE, PAGE_SIZE * 2, PAGE_SIZE * 5, PAGE_SIZE * 9];
        let prefetched = reader.prefetch(&bases, PAGE_SIZE);
        assert_eq!(prefetched, 4);
        assert!(
            inner.reads.load(std::sync::atomic::Ordering::Relaxed) >= 2,
            "large gaps split into multiple runs"
        );
        let mut buf = [0u8; 16];
        reader.read(Address(PAGE_SIZE * 5 + 7), &mut buf).unwrap();
        assert_eq!(&buf[..], &want(PAGE_SIZE * 5 + 7, 16)[..]);
        let (_, fetched, _, _) = reader.stats();
        assert_eq!(fetched, 4, "read served entirely from prefetched pages");
    }

    #[test]
    fn end_of_region_partial_read_forwards() {
        let inner = Linear::new(PAGE_SIZE + 100);
        let reader = FrameReader::new(&inner);
        let start = PAGE_SIZE + 90;
        let mut buf = vec![0u8; 16];
        let n = reader.read(Address(start), &mut buf).unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf[..10], &want(start, 10)[..]);
    }
}
