//! CPython object layout abstraction.
//!
//! The EVE client implements its UI in Python, so reading the UI tree means
//! decoding live CPython objects from process memory. The exact struct
//! layouts differ between CPython 2 (current clients) and CPython 3 (the
//! announced client migration), so all layout knowledge is isolated behind
//! the [`PythonLayout`] trait:
//!
//! - [`py2::Py2Layout`] — CPython 2.7, 64-bit, Windows (str/unicode UCS-2,
//!   open-addressing dicts, old-style instances). This is the layout of all
//!   shipping clients as of 2026-09.
//! - a future `py3` module will implement the same trait once the client
//!   migration lands, leaving the tree walker and semantic layer untouched.
//!
//! The trait deliberately exposes only *structural* reads (type pointers,
//! dict entry pairs, list items, scalar values). Policy — which dict keys to
//! keep, value whitelisting, output shaping — lives in [`crate::uitree`].

pub mod py2;

use std::collections::HashSet;

use crate::address::Address;
use crate::error::Result;
use crate::source::{MemoryRegion, MemorySource};

/// Structural CPython access for one object-layout generation.
pub trait PythonLayout: Send + Sync {
    fn name(&self) -> &'static str;

    /// Address of the type object referenced by an object header (`ob_type`).
    fn ob_type(&self, mem: &dyn MemorySource, object: Address) -> Result<Address>;

    /// `tp_name` of a type object: null-terminated bytes at the pointer
    /// stored at `type_object + tp_name_offset`, capped at `max_bytes`.
    fn type_name(
        &self,
        mem: &dyn MemorySource,
        type_object: Address,
        max_bytes: usize,
    ) -> Result<String>;

    /// Decode a `str` object (bytes + length header).
    fn read_str(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> Result<String>;

    /// Decode a `unicode` object (UTF-16 payload + length header).
    fn read_unicode(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> Result<String>;

    fn read_int(&self, mem: &dyn MemorySource, object: Address) -> Result<i64>;

    fn read_bool(&self, mem: &dyn MemorySource, object: Address) -> Result<bool>;

    fn read_float(&self, mem: &dyn MemorySource, object: Address) -> Result<f64>;

    /// Valid `(key object, value object)` pairs of a dict.
    /// Table sanity limits are enforced; `max_slots` caps the table size.
    fn dict_entries(
        &self,
        mem: &dyn MemorySource,
        dict: Address,
        max_slots: usize,
    ) -> Result<Vec<(Address, Address)>>;

    /// Key objects of a `set`/`frozenset` (tombstones removed).
    fn set_keys(
        &self,
        mem: &dyn MemorySource,
        set: Address,
        max_slots: usize,
    ) -> Result<Vec<Address>>;

    /// Item count of a list/tuple without reading the items.
    fn sequence_len(&self, mem: &dyn MemorySource, object: Address) -> Result<i64>;

    /// Items of a list object; fails when the item count exceeds `max_items`.
    fn list_items(
        &self,
        mem: &dyn MemorySource,
        list: Address,
        max_items: usize,
    ) -> Result<Vec<Address>>;

    /// Items of a tuple object (stored inline); fails above `max_items`.
    fn tuple_items(
        &self,
        mem: &dyn MemorySource,
        tuple: Address,
        max_items: usize,
    ) -> Result<Vec<Address>>;

    /// Address of an old-style instance `__dict__` stored at a fixed offset.
    fn instance_dict_direct(&self, mem: &dyn MemorySource, object: Address)
        -> Result<Address>;

    /// Address of an instance `__dict__` located via the type object's
    /// `tp_dictoffset`; `None` when the type has no instance dict.
    fn instance_dict_via_tp_dictoffset(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        type_object: Address,
    ) -> Result<Option<Address>>;

    // ------------------------------------------------------------------
    // Compound reads. Overriding these is a pure optimization: the
    /// default implementations compose the primitives above, so a future
    /// layout (e.g. Py3) stays correct even when it only implements the
    /// primitives — it just leaves the round-trip savings on the table.
    // ------------------------------------------------------------------

    /// One memory round trip returning the object's `ob_type` plus the
    /// raw qword at the layout's "second word" offset (Py2: +0x10 —
    /// the scalar value for int/bool/float, the old-style instance dict
    /// pointer for instance objects). Callers interpret the word by
    /// context after classifying via the type name.
    fn object_words(&self, mem: &dyn MemorySource, object: Address) -> Result<(Address, u64)> {
        let ob_type = self.ob_type(mem, object)?;
        let word = self.read_int(mem, object).map(|v| v as u64)?;
        Ok((ob_type, word))
    }

    /// One round trip for a list's `(length, items pointer)`. `None` when
    /// the layout provides no compound read — the caller falls back to
    /// [`Self::list_items`].
    fn list_words(&self, _mem: &dyn MemorySource, _list: Address) -> Result<Option<(i64, u64)>> {
        Ok(None)
    }

    // ------------------------------------------------------------------
    // Discovery scans (UIRoot location). These read whole committed regions
    // and are the expensive part of a cold read (~20 s live); they are part
    // of the layout because every step depends on header offsets.
    // ------------------------------------------------------------------

    /// Addresses of objects that are the `type` metatype itself
    /// (`ob_type` self-referential, `tp_name == "type"`).
    fn find_metatype_addresses(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
    ) -> HashSet<Address>;

    /// `(address, tp_name)` of every Python type object in memory.
    fn find_type_objects(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        metatypes: &HashSet<Address>,
        max_name_bytes: usize,
    ) -> Vec<(Address, String)>;

    /// Addresses of objects whose `ob_type` points into `types`.
    fn find_instances_of(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        types: &HashSet<Address>,
    ) -> Vec<Address>;

    /// Single-pass census: instance count per type object address.
    /// A drift-debugging tool ("which types exist right now, how many").
    fn count_instances_by_type(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        types: &HashSet<Address>,
    ) -> std::collections::HashMap<Address, usize>;
}
