//! # eve-memory
//!
//! Perception layer for EVE Online clients: discovers running game client
//! processes (any publisher/flavor), reads their memory, decodes the
//! CPython object graph, and extracts the UI tree.
//!
//! ## Layering
//!
//! ```text
//! discovery ──▶ LiveProcess ──▶ MemorySource ◀── DumpSource (zip replay)
//!                                │
//!                                ▼
//!                          PythonLayout        (trait: py2 now, py3 later)
//!                                │
//!                                ▼
//!                            TreeWalker ──▶ UiNode (+ Sanderling-compatible JSON)
//! ```
//!
//! - [`MemorySource`] unifies live reads and dump replay, making the whole
//!   stack testable offline against recorded samples.
//! - [`PythonLayout`] isolates CPython struct knowledge, so the announced
//!   client migration to Python 3 only requires a new layout
//!   implementation.
//! - [`UiReader`] is the facade: root discovery (cached) plus subtree
//!   reads.
//!
//! ## Multi-client
//!
//! [`discovery::discover_clients`] returns every running client with its
//! window; [`window::WindowHandle`] provides activation/minimize/restore
//! and client-area rectangles for later input mapping.

pub mod address;
pub mod cached_source;
pub mod discovery;
pub mod dump;
pub mod error;
pub mod flavor;
pub mod frame_reader;
pub mod pyobject;
pub mod rescache;
pub mod source;
pub mod uitree;
pub mod window;

use std::sync::Arc;

pub use address::{Address, AddressRange};
pub use cached_source::CachedSource;
pub use discovery::{discover_clients, GameClient};
pub use dump::{save_process_sample, DumpSource};
pub use error::{Error, Result};
pub use flavor::Flavor;
pub use frame_reader::FrameReader;
pub use pyobject::py2::Py2Layout;
pub use pyobject::PythonLayout;
pub use rescache::ResourceCache;
pub use source::{LiveProcess, MemoryRegion, MemorySource};
pub use uitree::{NodeValue, PersistentCaches, TreeLimits, TreeWalker, UiNode};
pub use window::WindowHandle;

/// Facade over one memory source: UI root discovery (cached) and tree reads.
///
/// A root search scans all committed memory (~20 s live, fast on dumps);
/// the winning root address is cached, so subsequent [`UiReader::read_tree`]
/// calls only walk the tree (~0.3 s live).
pub struct UiReader {
    source: Box<dyn MemorySource>,
    layout: Arc<dyn PythonLayout>,
    limits: TreeLimits,
    root: Option<Address>,
    /// Process-immutable cross-read caches (type names by type-object
    /// address, interned dict keys). Cleared together with the root on
    /// invalidation; see [`uitree::PersistentCaches`].
    caches: PersistentCaches,
    /// Page bases the last frame touched, used to bulk-prefetch the next
    /// frame (the page SET is stable frame-to-frame; contents are always
    /// re-read, so UI updates are never missed). Cleared on invalidation.
    known_pages: std::sync::Mutex<rustc_hash::FxHashSet<u64>>,
    /// Cap on `known_pages` — beyond this the set is rebuilt from the
    /// next frame alone (defensive against runaway growth in a fight).
    max_known_pages: usize,
    /// Opt-in subtree-level walk parallelism (negative on current tree
    /// sizes; a lever for much larger trees). See
    /// [`uitree::TreeWalker::set_parallel_walk`].
    parallel_walk: bool,
}

impl UiReader {
    /// Reader over a live game process.
    pub fn live(pid: u32) -> Result<UiReader> {
        Ok(UiReader {
            source: Box::new(LiveProcess::open(pid)?),
            layout: Arc::new(Py2Layout),
            limits: TreeLimits::default(),
            root: None,
            caches: PersistentCaches::default(),
            known_pages: std::sync::Mutex::new(rustc_hash::FxHashSet::default()),
            max_known_pages: 200_000,
            parallel_walk: false,
        })
    }

    /// Reader over a recorded sample archive.
    pub fn sample(path: &std::path::Path) -> Result<UiReader> {
        Ok(UiReader {
            source: Box::new(DumpSource::open_zip(path)?),
            layout: Arc::new(Py2Layout),
            limits: TreeLimits::default(),
            root: None,
            caches: PersistentCaches::default(),
            known_pages: std::sync::Mutex::new(rustc_hash::FxHashSet::default()),
            max_known_pages: 200_000,
            parallel_walk: false,
        })
    }

    pub fn with_limits(mut self, limits: TreeLimits) -> UiReader {
        self.limits = limits;
        self
    }

    pub fn source_description(&self) -> String {
        self.source.describe()
    }

    pub fn cached_root(&self) -> Option<Address> {
        self.root
    }

    /// Size of the persistent prefetch page set (diagnostics).
    pub fn known_page_count(&self) -> usize {
        self.known_pages.lock().unwrap().len()
    }

    /// Drop the cached root (e.g. after a client reload moved the UIRoot)
    /// and the process-immutable caches with it (defensive invalidation:
    /// a stale root and a reallocation event are the same trigger).
    pub fn invalidate_root(&mut self) {
        self.root = None;
        self.caches.clear();
        self.known_pages.lock().unwrap().clear();
    }

    /// Clear every cache (root, type names, dict keys).
    pub fn invalidate_all(&mut self) {
        self.invalidate_root();
    }

    /// Opt in to subtree-level walk parallelism (off by default).
    pub fn with_parallel_walk(mut self) -> UiReader {
        self.parallel_walk = true;
        self
    }

    /// Discover UIRoot candidate instances without reading trees.
    pub fn ui_root_candidates(&self) -> Result<Vec<Address>> {
        let regions = self.source.regions()?;
        let (ui_root_types, _type_objects) = self.find_ui_root_types(&regions)?;
        let instances = self
            .layout
            .find_instances_of(&*self.source, &regions, &ui_root_types);
        tracing::debug!(candidates = instances.len(), "instance scan");
        Ok(instances)
    }

    fn find_ui_root_types(
        &self,
        regions: &[MemoryRegion],
    ) -> Result<(std::collections::HashSet<Address>, Vec<(Address, String)>)> {
        let started = std::time::Instant::now();
        let metatypes = self.layout.find_metatype_addresses(&*self.source, regions);
        tracing::debug!(metatypes = metatypes.len(), elapsed = ?started.elapsed(), "metatype scan");
        let type_objects = self
            .layout
            .find_type_objects(&*self.source, regions, &metatypes, 256);
        let ui_root_types: std::collections::HashSet<Address> = type_objects
            .iter()
            .filter(|(_, name)| name == "UIRoot")
            .map(|(address, _)| *address)
            .collect();
        tracing::debug!(
            types = type_objects.len(),
            ui_root_types = ui_root_types.len(),
            elapsed = ?started.elapsed(),
            "type scan"
        );
        Ok((ui_root_types, type_objects))
    }

    /// List Python types present in memory with instance counts, most
    /// common first. Drift-debugging tool: reveals renames and new types
    /// after client updates.
    pub fn type_census(&self) -> Result<Vec<(String, usize)>> {
        let regions = self.source.regions()?;
        let (_ui_root_types, type_objects) = self.find_ui_root_types(&regions)?;
        let type_addresses: std::collections::HashSet<Address> =
            type_objects.iter().map(|(address, _)| *address).collect();
        let counts = self
            .layout
            .count_instances_by_type(&*self.source, &regions, &type_addresses);
        let mut census: Vec<(String, usize)> = type_objects
            .into_iter()
            .map(|(address, name)| {
                let count = counts.get(&address).copied().unwrap_or(0);
                (name, count)
            })
            .collect();
        census.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(census)
    }

    /// Discover the UIRoot and cache it (largest tree wins).
    pub fn find_ui_root(&mut self) -> Result<Address> {
        if let Some(root) = self.root {
            return Ok(root);
        }
        let candidates = self.ui_root_candidates()?;
        if candidates.is_empty() {
            return Err(Error::UiRootDiscoveryFailed {
                message: "no UIRoot instances found in memory".into(),
            });
        }
        let cached = CachedSource::new(&*self.source);
        let walker =
            TreeWalker::with_limits(&cached, &*self.layout, self.limits.clone(), &self.caches);
        let Some((root, _)) = walker.read_best_tree(&candidates)? else {
            return Err(Error::UiRootDiscoveryFailed {
                message: "all candidate trees unreadable".into(),
            });
        };
        self.root = Some(root);
        Ok(root)
    }

    /// Read the full UI tree from the (cached or discovered) UIRoot.
    pub fn read_tree(&mut self) -> Result<UiNode> {
        let root = self.find_ui_root()?;
        let tree = self.read_tree_at(root)?;
        Ok(tree)
    }

    /// Read the subtree rooted at a known address. Each call walks with
    /// a fresh [`FrameReader`] (one underlying read per 4 KiB page,
    /// prefetched from the previous frame's page set): the frame's data
    /// is a point-in-time snapshot and the next call re-reads every page
    /// — dynamic values are never carried across calls.
    pub fn read_tree_at(&self, root: Address) -> Result<UiNode> {
        self.read_tree_at_with_stats(root).map(|(tree, _)| tree)
    }

    /// [`Self::read_tree_at`] plus the frame-cache statistics of the
    /// walk: `(reads served, pages fetched, last-page hits, prefetched
    /// pages)`.
    pub fn read_tree_at_with_stats(
        &self,
        root: Address,
    ) -> Result<(UiNode, (u64, u64, u64, u64))> {
        let frame = FrameReader::new(&*self.source);
        // Overlapped prefetch: replay the previous frame's page set on a
        // concurrent thread while the walk starts immediately (racing
        // fill — misses read pages themselves; a doubly-read page costs
        // one redundant 4 KiB read, never correctness, since both happen
        // within this frame). The page SET is reused across frames; the
        // CONTENTS are always read fresh, so UI updates land exactly
        // like any other read.
        let known_snapshot: Vec<u64> =
            self.known_pages.lock().unwrap().iter().copied().collect();
        let tree = std::thread::scope(|scope| {
            if !known_snapshot.is_empty() {
                scope.spawn(|| frame.prefetch(&known_snapshot, 4 * 1024));
            }
            let walker =
                TreeWalker::with_limits(&frame, &*self.layout, self.limits.clone(), &self.caches);
            if self.parallel_walk {
                walker.set_parallel_walk();
                frame.set_parallel();
            }
            walker.read_tree_from(root)
        })?
        .ok_or_else(|| Error::UiRootDiscoveryFailed {
            message: format!("tree at {root} unreadable or pruned"),
        })?;
        let stats = frame.stats();
        // Roll the page set forward to this frame's touch set. Above the
        // defensive cap the set is simply rebuilt next frame.
        let touched = frame.touched_pages();
        let mut known = self.known_pages.lock().unwrap();
        if touched.len() <= self.max_known_pages {
            known.clear();
            known.extend(touched);
        } else {
            known.clear();
        }
        Ok((tree, stats))
    }
}
