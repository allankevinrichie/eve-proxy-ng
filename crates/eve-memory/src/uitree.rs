//! UI tree extraction: policy layer on top of a [`PythonLayout`].
//!
//! Walks the CPython object graph from a root node and produces a typed
//! [`UiNode`] tree containing only the dict entries relevant for game
//! semantics (the whitelist below), following the structure the Sanderling
//! project established:
//!
//! - `_display == false` prunes the whole subtree;
//! - `NoneType`-valued entries are dropped;
//! - `children` chains resolve through `_childrenObjects` (with one
//!   `PyChildrenList` indirection);
//! - specialized readers expose known game types (`PyColor`, `Bunch`,
//!   `Link`, `InteractionState`, `Tr2GlyphString`, `list`, `set`);
//! - every non-whitelisted key is preserved in `other_entries_keys` so new
//!   UI attributes stay observable.
//!
//! The JSON serialization is wire-compatible with Sanderling's
//! `read-memory-64-bit` output (addresses as strings, same field names),
//! which allows golden-testing a Rust read against the reference reader.

use std::collections::BTreeMap;
use rustc_hash::{FxHashMap, FxHashSet};

use serde::Serialize;

use crate::address::Address;
use crate::error::{Error, Result};
use crate::pyobject::PythonLayout;
use crate::source::MemorySource;

/// Dict keys preserved in `dict_entries` — the game knowledge of the
/// perception layer. Order/grouping mirrors the reference implementation,
/// including the date comments, so drift can be audited.
pub const DICT_KEYS_OF_INTEREST: &[&str] = &[
    "_top",
    "_left",
    "_width",
    "_height",
    "_displayX",
    "_displayY",
    "_displayHeight",
    "_displayWidth",
    "_name",
    "_text",
    "_setText",
    "children",
    "texturePath",
    "_bgTexturePath",
    "_hint",
    "_display",
    // HPGauges
    "lastShield",
    "lastArmor",
    "lastStructure",
    // ShipHudSpriteGauge
    "_lastValue",
    // ModuleButton
    "ramp_active",
    // ShipModuleButtonRamps transforms
    "_rotation",
    // OverviewEntry iconSprite
    "_color",
    // SE_TextlineCore
    "_sr",
    // `_sr` Bunch
    "htmlstr",
    // 2026-07-27: rich-text link anchors (`Link` under `links`, `href`)
    "attrs",
    // 2026-07-27: inside `attrs` Bunch: anchor text and target
    "linkText",
    "url",
    // 2026-07-26: `_sr.paragraphs` = list of Tr2GlyphString
    "paragraphs",
    // 2026-07-26: editable-vs-readonly text area flag
    "readonly",
    // 2023-01-03: Photon UI
    "_texturePath",
    "_opacity",
    "_bgColor",
    "isExpanded",
    // 2026-07-25: greyed-out state
    "enabled",
    "isDisabled",
    "_enabled",
    "_interaction_state",
    // 2026-07-25: selection / checked state
    "_selected",
    "isSelected",
    "_checked",
    "isChecked",
    // 2026-07-26: util menu rows
    // 2026-07-25: sort direction on ColumnHeader (1 asc, -1 desc)
    "_direction",
    // 2026-07-27: Carbon hit-testing (0 blocks input, 1 takes it, 2 passes)
    "_pickState",
];

/// Readers fail with these literal strings (kept wire-compatible with the
/// reference implementation; the semantic layer discards them).
const FAILED_STRING_BYTES: &str = "Failed to read string bytes.";
const FAILED_INT_OBJECT: &str = "Failed to read int object memory.";
const FAILED_LIST_OBJECT: &str = "Failed to read list object memory.";
const FAILED_LIST_ITEMS: &str = "Failed to read list items.";
const FAILED_SET_SLOTS: &str = "Failed to read set slots memory.";
const FAILED_SET_TOO_LARGE: &str = "Set too large.";
const FAILED_PY_COLOR: &str = "Failed to read pyColorObjectMemory.";
const FAILED_INTERACTION_STATE: &str = "Failed to read InteractionState object memory.";
const FAILED_INTERACTION_STATE_DICT: &str =
    "Failed to read InteractionState dictionary entries.";
const INTERACTION_STATE_NO_NAME: &str = "InteractionState carries no _name_.";

/// Value-recursion guard for nested containers (`Bunch`, `list`, `set`,
/// `Link`). The reference implementation recurses unguarded; we cap to
/// survive hostile/corrupt snapshots.
const MAX_VALUE_DEPTH: u32 = 12;

/// Specialized `list` reader cap.
const SPECIALIZED_LIST_MAX_ITEMS: usize = 200;

/// Tuning limits of one tree read. Defaults mirror the reference reader.
#[derive(Clone, Debug)]
pub struct TreeLimits {
    pub max_depth: u32,
    pub dict_slots: usize,
    pub string_chars: usize,
    pub key_chars: usize,
    pub children_items: usize,
    pub set_slots: usize,
    /// Collect non-whitelisted keys into `other_entries_keys`?
    pub keep_other_keys: bool,
}

impl Default for TreeLimits {
    fn default() -> Self {
        TreeLimits {
            max_depth: 99,
            dict_slots: 10_000,
            string_chars: 0x40000,
            key_chars: 4_000,
            children_items: 4_000,
            set_slots: 10_000,
            keep_other_keys: true,
        }
    }
}

/// A decoded value of a dict entry of interest.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Float(f64),
    /// Integer wider than 32 bits, as emitted by the reference format.
    WideInt { int: i64, int_low32: i32 },
    /// `PyColor` with channels as integer percent, when readable.
    Color {
        a: Option<i32>,
        r: Option<i32>,
        g: Option<i32>,
        b: Option<i32>,
    },
    /// `Bunch` read as a dict, whitelist-filtered one level deeper.
    Bunch {
        entries_of_interest: BTreeMap<String, NodeValue>,
        other_keys: Option<Vec<String>>,
    },
    /// `list` / `set` element arrays.
    List(Vec<NodeValue>),
    /// Any other object: address + type name.
    Ref {
        address: Address,
        type_name: String,
    },
    /// `Link`: a full nested node (decoded from the dict it references).
    Node(Box<UiNode>),
}

impl Serialize for NodeValue {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            NodeValue::Str(text) => serializer.serialize_str(text),
            NodeValue::Int(value) => {
                if let Ok(small) = i32::try_from(*value) {
                    serializer.serialize_i32(small)
                } else {
                    NodeValue::WideInt {
                        int: *value,
                        int_low32: *value as u32 as i32,
                    }
                    .serialize(serializer)
                }
            }
            NodeValue::Bool(value) => serializer.serialize_bool(*value),
            NodeValue::Float(value) => {
                if value.is_finite() {
                    serializer.serialize_f64(*value)
                } else {
                    serializer.serialize_none()
                }
            }
            NodeValue::WideInt { int, int_low32 } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("int", &int.to_string())?;
                map.serialize_entry("int_low32", int_low32)?;
                map.end()
            }
            NodeValue::Color { a, r, g, b } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("aPercent", a)?;
                map.serialize_entry("rPercent", r)?;
                map.serialize_entry("gPercent", g)?;
                map.serialize_entry("bPercent", b)?;
                map.end()
            }
            NodeValue::Bunch {
                entries_of_interest,
                other_keys,
            } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("entriesOfInterest", entries_of_interest)?;
                map.serialize_entry("otherDictEntriesKeys", other_keys)?;
                map.end()
            }
            NodeValue::List(values) => values.serialize(serializer),
            NodeValue::Ref { address, type_name } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("address", &address.0.to_string())?;
                map.serialize_entry("pythonObjectTypeName", type_name)?;
                map.end()
            }
            NodeValue::Node(node) => node.serialize(serializer),
        }
    }
}

/// One node of the extracted UI tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UiNode {
    #[serde(rename = "pythonObjectAddress", serialize_with = "serialize_address_as_string")]
    pub address: Address,
    #[serde(rename = "pythonObjectTypeName")]
    pub type_name: String,
    #[serde(rename = "dictEntriesOfInterest")]
    pub entries: BTreeMap<String, NodeValue>,
    #[serde(rename = "otherDictEntriesKeys")]
    pub other_entries_keys: Option<Vec<String>>,
    /// `None` only for `Link` pseudo-nodes; real nodes always have a list.
    pub children: Option<Vec<UiNode>>,
}

fn serialize_address_as_string<S: serde::Serializer>(
    address: &Address,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&address.0.to_string())
}

/// Cross-read caches for process-immutable data. Type names are keyed by
/// the **type object address** (C static structures, unchanged for the
/// process lifetime) and dict keys by the interned string object address.
/// Dynamic values (text, coords, state) are NEVER cached here — every
/// frame re-reads them, so UI updates cannot be missed. `clear` provides
/// the defensive invalidation path wired into root invalidation.
/// Mutex (not RefCell) only to keep `UiReader` shareable with the Python
/// binding; walkers are single-threaded. `Arc<str>` values make hot-path
/// lookups a cheap refcount bump instead of a string copy.
#[derive(Default)]
pub struct PersistentCaches {
    pub type_names: std::sync::Mutex<FxHashMap<Address, Option<std::sync::Arc<str>>>>,
    pub dict_keys: std::sync::Mutex<FxHashMap<Address, std::sync::Arc<str>>>,
}

impl PersistentCaches {
    pub fn clear(&self) {
        self.type_names.lock().unwrap().clear();
        self.dict_keys.lock().unwrap().clear();
    }
}

/// Walks a memory source with one layout and produces `UiNode` trees.
pub struct TreeWalker<'a> {
    mem: &'a dyn MemorySource,
    layout: &'a dyn PythonLayout,
    limits: TreeLimits,
    keys_of_interest: FxHashSet<&'static str>,
    /// Shared across reads of one process; see [`PersistentCaches`].
    caches: &'a PersistentCaches,
    /// Opt-in subtree-level walk parallelism. Off by default: measured
    /// NEGATIVE on the current tree sizes (~10µs per node is too fine
    /// for task overhead, and parallel mode drops the single-thread
    /// last-page fast path). Meant for much larger trees (big fights);
    /// enable together with [`crate::FrameReader::set_parallel`].
    parallel_walk: std::sync::atomic::AtomicBool,
}

impl<'a> TreeWalker<'a> {
    pub fn with_limits(
        mem: &'a dyn MemorySource,
        layout: &'a dyn PythonLayout,
        limits: TreeLimits,
        caches: &'a PersistentCaches,
    ) -> TreeWalker<'a> {
        TreeWalker {
            mem,
            layout,
            keys_of_interest: DICT_KEYS_OF_INTEREST.iter().copied().collect(),
            limits,
            caches,
            parallel_walk: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Enable subtree-level walk parallelism (see field docs).
    pub fn set_parallel_walk(&self) {
        self.parallel_walk.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Read the tree rooted at `root`. Returns `Ok(None)` when the root is
    /// unreadable or pruned (`_display == false`).
    pub fn read_tree_from(&self, root: Address) -> Result<Option<UiNode>> {
        self.node_from_object(root, self.limits.max_depth)
    }

    /// Read every candidate tree and keep the one with the most nodes —
    /// the selection rule of the reference reader for UIRoot candidates.
    pub fn read_best_tree(&self, candidates: &[Address]) -> Result<Option<(Address, UiNode)>> {
        let mut best: Option<(Address, UiNode, usize)> = None;
        for &candidate in candidates {
            let Some(tree) = self.read_tree_from(candidate)? else {
                continue;
            };
            let count = count_nodes(&tree);
            tracing::debug!(root = %candidate, nodes = count, "candidate tree");
            match &best {
                Some((_, _, best_count)) if *best_count >= count => {}
                _ => best = Some((candidate, tree, count)),
            }
        }
        Ok(best.map(|(address, tree, _)| (address, tree)))
    }

    // -- internals ------------------------------------------------------

    /// Type name for a type OBJECT address (cache key: type object).
    fn type_name_of_type_object(&self, type_object: Address) -> Option<std::sync::Arc<str>> {
        if let Some(cached) = self.caches.type_names.lock().unwrap().get(&type_object) {
            return cached.clone();
        }
        let resolved = self
            .layout
            .type_name(self.mem, type_object, 100)
            .ok()
            .filter(|name| !name.is_empty())
            .map(|name| std::sync::Arc::from(name.into_boxed_str()));
        self.caches
            .type_names
            .lock()
            .unwrap()
            .insert(type_object, resolved.clone());
        resolved
    }

    /// Type name for an instance address: one compound header read plus
    /// the (cached) type-object lookup.
    fn type_name_of(&self, object: Address) -> Option<std::sync::Arc<str>> {
        let (type_object, _) = self.layout.object_words(self.mem, object).ok()?;
        self.type_name_of_type_object(type_object)
    }

    fn dict_key_string(&self, key: Address) -> Option<std::sync::Arc<str>> {
        if let Some(cached) = self.caches.dict_keys.lock().unwrap().get(&key) {
            return Some(cached.clone());
        }
        let decoded: Option<std::sync::Arc<str>> = self
            .layout
            .read_str(self.mem, key, self.limits.key_chars)
            .ok()
            .map(|text| std::sync::Arc::from(text.into_boxed_str()));
        if let Some(text) = &decoded {
            self.caches.dict_keys.lock().unwrap().insert(key, text.clone());
        }
        decoded
    }

    fn dict_pairs(&self, dict: Address) -> Vec<(Address, Address)> {
        self.layout
            .dict_entries(self.mem, dict, self.limits.dict_slots)
            .unwrap_or_default()
    }

    fn node_from_object(&self, address: Address, depth: u32) -> Result<Option<UiNode>> {
        if depth == 0 {
            return Ok(None);
        }
        // One compound read: ob_type plus the instance-dict qword.
        let (type_object, dict_word) = match self.layout.object_words(self.mem, address) {
            Ok(words) => words,
            Err(_) => return Ok(None),
        };
        let Some(type_name) = self.type_name_of_type_object(type_object) else {
            return Ok(None);
        };

        let mut entries: BTreeMap<String, NodeValue> = BTreeMap::new();
        let mut other_keys: Vec<String> = Vec::new();
        let mut children_entry_value: Option<Address> = None;
        let mut display = true;

        let dict = Address(dict_word);
        if !dict.is_null() {
            for (key, value) in self.dict_pairs(dict) {
                let Some(key_text) = self.dict_key_string(key) else {
                    continue;
                };
                // Whitelist gate BEFORE touching the value: entries
                // outside the ~40-key whitelist only contribute their
                // key name to `other_entries_keys`. `_display` and
                // `children` are whitelist members.
                if !self.keys_of_interest.contains(&*key_text) {
                    if self.limits.keep_other_keys {
                        other_keys.push(key_text.to_string());
                    }
                    continue;
                }
                // Compound read gives the value's type object AND its
                // scalar qword in one round trip.
                let (value_type_object, word) =
                    match self.layout.object_words(self.mem, value) {
                        Ok(words) => words,
                        Err(_) => continue,
                    };
                let Some(value_type) = self.type_name_of_type_object(value_type_object) else {
                    continue;
                };
                if &*value_type == "NoneType" {
                    continue;
                }
                let decoded = match &*value_type {
                    "int" => NodeValue::Int(word as i64),
                    "bool" => NodeValue::Bool(word != 0),
                    "float" => NodeValue::Float(f64::from_bits(word)),
                    _ => self.decode_value(value, &value_type, 0),
                };
                if &*key_text == "_display" && decoded == NodeValue::Bool(false) {
                    display = false;
                    break;
                }
                if &*key_text == "children" {
                    children_entry_value = Some(value);
                }
                entries.insert(key_text.to_string(), decoded);
            }
        }
        if !display {
            return Ok(None);
        }

        let children = self
            .children_of(children_entry_value, depth)?
            .unwrap_or_default();

        Ok(Some(UiNode {
            address,
            type_name: type_name.to_string(),
            entries,
            other_entries_keys: if self.limits.keep_other_keys {
                Some(other_keys)
            } else {
                None
            },
            children: Some(children),
        }))
    }

    /// Resolve the `children` chain: children object -> instance dict ->
    /// `_childrenObjects` -> (one `PyChildrenList` indirection) -> list.
    fn children_of(
        &self,
        children_entry_value: Option<Address>,
        depth: u32,
    ) -> Result<Option<Vec<UiNode>>> {
        let Some(mut current) = children_entry_value else {
            return Ok(None);
        };
        let children_list = 'resolve: {
            for _ in 0..2 {
                // Compound read: the children object's instance-dict qword.
                let dict = match self.layout.object_words(self.mem, current) {
                    Ok((_, word)) => Address(word),
                    Err(_) => break 'resolve None,
                };
                if dict.is_null() {
                    break 'resolve None;
                }
                let value = self
                    .dict_pairs(dict)
                    .into_iter()
                    .find(|(key, _)| {
                        self.dict_key_string(*key).as_deref() == Some("_childrenObjects")
                    })
                    .map(|(_, value)| value);
                let Some(value) = value else {
                    break 'resolve None;
                };
                if self.type_name_of(value).as_deref() == Some("PyChildrenList") {
                    current = value;
                    continue;
                }
                break 'resolve Some(value);
            }
            None
        };
        let Some(children_list) = children_list else {
            return Ok(None);
        };
        let items = self.children_list_items(children_list)?;
        // Subtree-level parallelism, coarse-grained: only the top few
        // tree levels fan out to rayon, where each subtree is a
        // milliseconds-scale task that amortizes scheduling. Deeper
        // levels stay serial — per-node work (~10µs) is too fine for
        // task overhead (measured: everywhere-forking was slower than
        // the serial walk). The walker is Sync and the FrameReader page
        // table is sharded. The indexed collect preserves child
        // (z-)order.
        if self.parallel_walk.load(std::sync::atomic::Ordering::Acquire)
            && items.len() >= 3
            && depth > 95
        {
            use rayon::prelude::*;
            let results: Vec<Result<Option<UiNode>>> = items
                .into_par_iter()
                .map(|item| self.node_from_object(item, depth - 1))
                .collect();
            let mut nodes = Vec::with_capacity(results.len());
            for result in results {
                if let Some(node) = result? {
                    nodes.push(node);
                }
            }
            return Ok(Some(nodes));
        }
        let mut nodes = Vec::with_capacity(items.len());
        for item in items {
            if let Some(node) = self.node_from_object(item, depth - 1)? {
                nodes.push(node);
            }
        }
        Ok(Some(nodes))
    }

    /// Read the children list's item pointers, preferring the compound
    /// `(length, items pointer)` header read and falling back to
    /// `list_items` for layouts without one.
    fn children_list_items(&self, list: Address) -> Result<Vec<Address>> {
        if let Ok(Some((len, items_pointer))) = self.layout.list_words(self.mem, list) {
            if len < 0 {
                return Err(Error::InvalidPyObject {
                    address: list.0,
                    message: format!("children list: negative length {len}"),
                });
            }
            if len as usize > self.limits.children_items {
                return Err(Error::InvalidPyObject {
                    address: list.0,
                    message: format!("children list too large: {len} items"),
                });
            }
            if items_pointer == 0 {
                return Ok(Vec::new());
            }
            let mut bytes = vec![0u8; (len * 8) as usize];
            self.mem.read_exact(Address(items_pointer), &mut bytes).map_err(|e| {
                Error::InvalidPyObject {
                    address: list.0,
                    message: format!("children list: {e}"),
                }
            })?;
            return Ok(crate::pyobject::py2::as_words(&bytes).map(Address).collect());
        }
        self.layout
            .list_items(self.mem, list, self.limits.children_items)
            .map_err(|e| Error::InvalidPyObject {
                address: list.0,
                message: format!("children list: {e}"),
            })
    }

    fn decode_value(&self, address: Address, type_name: &str, value_depth: u32) -> NodeValue {
        if value_depth > MAX_VALUE_DEPTH {
            return NodeValue::Ref {
                address,
                type_name: type_name.to_string(),
            };
        }
        match type_name {
            "str" => self
                .layout
                .read_str(self.mem, address, self.limits.string_chars)
                .map(NodeValue::Str)
                .unwrap_or(NodeValue::Str(FAILED_STRING_BYTES.to_string())),
            "unicode" => self
                .layout
                .read_unicode(self.mem, address, self.limits.string_chars)
                .map(NodeValue::Str)
                .unwrap_or(NodeValue::Str(FAILED_STRING_BYTES.to_string())),
            "int" => self
                .layout
                .read_int(self.mem, address)
                .map(NodeValue::Int)
                .unwrap_or(NodeValue::Str(FAILED_INT_OBJECT.to_string())),
            "bool" => match self.layout.read_bool(self.mem, address) {
                Ok(value) => NodeValue::Bool(value),
                Err(_) => NodeValue::Ref {
                    address,
                    type_name: type_name.to_string(),
                },
            },
            "float" => match self.layout.read_float(self.mem, address) {
                Ok(value) => NodeValue::Float(value),
                Err(_) => NodeValue::Ref {
                    address,
                    type_name: type_name.to_string(),
                },
            },
            "PyColor" => self.decode_py_color(address),
            "Bunch" => self.decode_bunch(address, value_depth),
            "Link" => self.decode_link(address, value_depth),
            "set" | "frozenset" => self.decode_set(address, value_depth),
            "InteractionState" => self.decode_interaction_state(address),
            "list" => self.decode_list(address, value_depth),
            "Tr2GlyphString" => self.decode_glyph_string(address),
            _ => NodeValue::Ref {
                address,
                type_name: type_name.to_string(),
            },
        }
    }

    fn decode_py_color(&self, address: Address) -> NodeValue {
        let channel_percent = |entries: &[(Address, Address)], key: &str| -> Option<i32> {
            let value = entries
                .iter()
                .find(|(k, _)| self.dict_key_string(*k).as_deref() == Some(key))
                .map(|(_, v)| *v)?;
            self.layout
                .read_float(self.mem, value)
                .ok()
                .map(|f| (f * 100.0) as i32)
        };
        let Ok(dict) = self.layout.instance_dict_direct(self.mem, address) else {
            return NodeValue::Str(FAILED_PY_COLOR.to_string());
        };
        if dict.is_null() {
            return NodeValue::Str(FAILED_PY_COLOR.to_string());
        }
        let entries = self.dict_pairs(dict);
        if entries.is_empty() {
            return NodeValue::Str(FAILED_PY_COLOR.to_string());
        }
        NodeValue::Color {
            a: channel_percent(&entries, "_a"),
            r: channel_percent(&entries, "_r"),
            g: channel_percent(&entries, "_g"),
            b: channel_percent(&entries, "_b"),
        }
    }

    fn decode_bunch(&self, address: Address, value_depth: u32) -> NodeValue {
        let mut entries_of_interest = BTreeMap::new();
        let mut other_keys = Vec::new();
        for (key, value) in self.dict_pairs(address) {
            let Some(key_text) = self.dict_key_string(key) else {
                continue;
            };
            let Some(value_type) = self.type_name_of(value) else {
                continue;
            };
            if &*value_type == "NoneType" {
                continue;
            }
            let decoded = self.decode_value(value, &value_type, value_depth + 1);
            if self.keys_of_interest.contains(&*key_text) {
                entries_of_interest.insert(key_text.to_string(), decoded);
            } else if self.limits.keep_other_keys {
                other_keys.push(key_text.to_string());
            }
        }
        NodeValue::Bunch {
            entries_of_interest,
            other_keys: if self.limits.keep_other_keys {
                Some(other_keys)
            } else {
                None
            },
        }
    }

    fn decode_link(&self, address: Address, value_depth: u32) -> NodeValue {
        // The Link object references a dict somewhere in its first 0x40
        // bytes; take the first qword pointing to an actual dict.
        let mut buffer = [0u8; 0x40];
        if self.mem.read_exact(address, &mut buffer).is_err() {
            return NodeValue::Ref {
                address,
                type_name: "Link".to_string(),
            };
        }
        for offset in (0..0x40 - 8).step_by(8) {
            let pointer = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
            if pointer == 0 {
                continue;
            }
            let dict_address = Address(pointer);
            if self.type_name_of(dict_address).as_deref() != Some("dict") {
                continue;
            }
            return self.link_node_from_dict(dict_address, value_depth);
        }
        NodeValue::Ref {
            address,
            type_name: "Link".to_string(),
        }
    }

    /// A `Link` renders as a node whose entries are the *full* dict content
    /// (no whitelist) with no children.
    fn link_node_from_dict(&self, dict_address: Address, value_depth: u32) -> NodeValue {
        let mut entries = BTreeMap::new();
        for (key, value) in self.dict_pairs(dict_address) {
            let Some(key_text) = self.dict_key_string(key) else {
                continue;
            };
            let Some(value_type) = self.type_name_of(value) else {
                continue;
            };
            if &*value_type == "NoneType" {
                continue;
            }
            entries.insert(key_text.to_string(), self.decode_value(value, &value_type, value_depth + 1));
        }
        NodeValue::Node(Box::new(UiNode {
            address: dict_address,
            type_name: "dict".to_string(),
            entries,
            other_entries_keys: None,
            children: None,
        }))
    }

    fn decode_set(&self, address: Address, value_depth: u32) -> NodeValue {
        let keys = match self.layout.set_keys(self.mem, address, self.limits.set_slots) {
            Ok(keys) => keys,
            Err(Error::InvalidPyObject { message, .. }) if message.starts_with("set too large") => {
                return NodeValue::Str(FAILED_SET_TOO_LARGE.to_string())
            }
            Err(_) => return NodeValue::Str(FAILED_SET_SLOTS.to_string()),
        };
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            let type_name = self.type_name_of(key).unwrap_or_default();
            values.push(self.decode_value(key, &type_name, value_depth + 1));
        }
        NodeValue::List(values)
    }

    fn decode_interaction_state(&self, address: Address) -> NodeValue {
        let Ok(dict) = self.layout.instance_dict_direct(self.mem, address) else {
            return NodeValue::Str(FAILED_INTERACTION_STATE.to_string());
        };
        if dict.is_null() {
            return NodeValue::Str(FAILED_INTERACTION_STATE_DICT.to_string());
        }
        let name = self
            .dict_pairs(dict)
            .into_iter()
            .find(|(key, _)| self.dict_key_string(*key).as_deref() == Some("_name"))
            .and_then(|(_, value)| self.layout.read_str(self.mem, value, 100).ok());
        match name {
            Some(name) => NodeValue::Str(name),
            None => NodeValue::Str(INTERACTION_STATE_NO_NAME.to_string()),
        }
    }

    fn decode_list(&self, address: Address, value_depth: u32) -> NodeValue {
        let length = match self.layout.sequence_len(self.mem, address) {
            Ok(length) => length,
            Err(_) => return NodeValue::Str(FAILED_LIST_OBJECT.to_string()),
        };
        if length as usize > SPECIALIZED_LIST_MAX_ITEMS {
            return NodeValue::Str(format!("list of {length} items (not expanded)"));
        }
        let items = match self
            .layout
            .list_items(self.mem, address, SPECIALIZED_LIST_MAX_ITEMS)
        {
            Ok(items) => items,
            Err(_) => return NodeValue::Str(FAILED_LIST_ITEMS.to_string()),
        };
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            let type_name = self.type_name_of(item).unwrap_or_default();
            values.push(self.decode_value(item, &type_name, value_depth + 1));
        }
        NodeValue::List(values)
    }

    fn decode_glyph_string(&self, address: Address) -> NodeValue {
        let text = (|| -> Option<String> {
            let type_object = self.layout.ob_type(self.mem, address).ok()?;
            let dict = self
                .layout
                .instance_dict_via_tp_dictoffset(self.mem, address, type_object)
                .ok()
                .flatten()?;
            let value = self
                .dict_pairs(dict)
                .into_iter()
                .find(|(key, _)| self.dict_key_string(*key).as_deref() == Some("text"))
                .map(|(_, value)| value)?;
            self.layout
                .read_str(self.mem, value, self.limits.string_chars)
                .ok()
        })();
        NodeValue::Str(text.unwrap_or_else(|| FAILED_STRING_BYTES.to_string()))
    }
}

/// Total node count of a tree (iterative, no recursion).
pub fn count_nodes(root: &UiNode) -> usize {
    let mut stack = vec![root];
    let mut count = 0;
    while let Some(node) = stack.pop() {
        count += 1;
        if let Some(children) = &node.children {
            stack.extend(children.iter());
        }
    }
    count
}
