//! Frame-to-frame incremental encoding of [`UiSnapshot`] streams.
//!
//! A snapshot is split into its top-level sections (~20 named windows/
//! panels). Each frame only the sections whose serialized value differs
//! from the previous frame are written; `interaction_elements` — by far
//! the largest section — is diffed per element keyed by node address.
//! Every line is self-describing JSON (NDJSON), so a consumer replays a
//! stream by applying `snapshot_full` then folding `snapshot_delta`s.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use eve_semantics::UiSnapshot;

/// Section treated as a keyed list: diffs report added/updated elements
/// instead of rewriting the whole array.
pub const KEYED_SECTION: &str = "interaction_elements";

/// Split a snapshot into named sections keyed by the `UiSnapshot` field
/// names. Values are the serialized form (identical to the corresponding
/// fields of a full snapshot).
pub fn snapshot_sections(snap: &UiSnapshot) -> BTreeMap<&'static str, Value> {
    fn ser<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or(Value::Null)
    }
    let mut m = BTreeMap::new();
    m.insert("flavor", ser(&snap.flavor));
    m.insert("node_count", ser(&snap.node_count));
    m.insert("game_state", ser(&snap.game_state));
    m.insert("ship_ui", ser(&snap.ship_ui));
    m.insert("module_button_tooltip", ser(&snap.module_button_tooltip));
    m.insert("overview_windows", ser(&snap.overview_windows));
    m.insert("context_menus", ser(&snap.context_menus));
    m.insert("util_menus", ser(&snap.util_menus));
    m.insert("inventory_windows", ser(&snap.inventory_windows));
    m.insert("message_boxes", ser(&snap.message_boxes));
    m.insert("neocom", ser(&snap.neocom));
    m.insert("info_panels", ser(&snap.info_panels));
    m.insert("selected_item_window", ser(&snap.selected_item_window));
    m.insert("chat_window_stacks", ser(&snap.chat_window_stacks));
    m.insert("fitting_window", ser(&snap.fitting_window));
    m.insert("station_window", ser(&snap.station_window));
    m.insert("scrollable_views", ser(&snap.scrollable_views));
    m.insert("layers", ser(&snap.layers));
    m.insert("other_windows", ser(&snap.other_windows));
    m.insert(KEYED_SECTION, ser(&snap.interaction_elements));
    m
}

/// Result of diffing two section maps.
#[derive(Debug, Default, PartialEq)]
pub struct DeltaSections {
    /// Sections whose value changed; the keyed section carries its keyed
    /// diff object instead of the full array.
    pub changed: Map<String, Value>,
    /// Sections that were non-null before and null now.
    pub cleared: Vec<String>,
}

impl DeltaSections {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.cleared.is_empty()
    }
}

/// Diff next against prev section-by-section. `prev` is `None` for the
/// first frame (callers emit a full snapshot instead).
pub fn diff_sections(
    prev: Option<&BTreeMap<&'static str, Value>>,
    next: &BTreeMap<&'static str, Value>,
) -> DeltaSections {
    let Some(prev) = prev else {
        return DeltaSections::default();
    };
    let mut out = DeltaSections::default();
    for (name, new_val) in next {
        let old_val = prev.get(*name).cloned().unwrap_or(Value::Null);
        if *new_val == old_val {
            continue;
        }
        if *name == KEYED_SECTION {
            let prev_arr = old_val.as_array().cloned().unwrap_or_default();
            let next_arr = new_val.as_array().cloned().unwrap_or_default();
            out.changed
                .insert((*name).to_string(), keyed_diff(&prev_arr, &next_arr));
        } else {
            out.changed.insert((*name).to_string(), new_val.clone());
            if !old_val.is_null() && new_val.is_null() {
                out.cleared.push((*name).to_string());
            }
        }
    }
    out
}

/// Per-element diff of two arrays of objects keyed by their `address`
/// field. Output shape:
/// `{ "added": [...], "updated": [...], "removed": ["addr"...],
///    "order": ["addr"...] }` — `order` preserves z-order (tree order).
pub fn keyed_diff(prev: &[Value], next: &[Value]) -> Value {
    let key_of =
        |v: &Value| v.get("address").and_then(Value::as_str).unwrap_or("?").to_string();
    let prev_map: BTreeMap<String, &Value> = prev.iter().map(|v| (key_of(v), v)).collect();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut order = Vec::new();
    for v in next {
        let key = key_of(v);
        order.push(Value::String(key.clone()));
        match prev_map.get(&key) {
            Some(old) if **old != *v => updated.push(v.clone()),
            Some(_) => {}
            None => added.push(v.clone()),
        }
    }
    let next_keys: std::collections::HashSet<&str> = next
        .iter()
        .map(|v| v.get("address").and_then(Value::as_str).unwrap_or("?"))
        .collect();
    let removed: Vec<Value> = prev
        .iter()
        .map(|v| key_of(v))
        .filter(|k| !next_keys.contains(k.as_str()))
        .map(Value::String)
        .collect();
    Value::Object(Map::from_iter([
        ("added".into(), Value::Array(added)),
        ("updated".into(), Value::Array(updated)),
        ("removed".into(), Value::Array(removed)),
        ("order".into(), Value::Array(order)),
    ]))
}

/// Rolling differ producing one jsonl line value per pushed snapshot.
pub struct SnapshotDiffer {
    prev: Option<BTreeMap<&'static str, Value>>,
    pub frames: u64,
}

impl SnapshotDiffer {
    pub fn new() -> Self {
        Self { prev: None, frames: 0 }
    }

    /// Build the observation line (without timing fields — the caller
    /// injects `t_ms`/`read_ms`) for the next snapshot.
    pub fn next_line(&mut self, snap: &UiSnapshot) -> Value {
        let sections = snapshot_sections(snap);
        let frame = self.frames;
        self.frames += 1;
        let record = match &self.prev {
            None => Value::Object(Map::from_iter([
                ("frame".into(), Value::from(frame)),
                ("kind".into(), Value::from("snapshot_full")),
                ("snapshot".into(), flatten_sections(&sections)),
            ])),
            Some(prev) => {
                let delta = diff_sections(Some(prev), &sections);
                Value::Object(Map::from_iter([
                    ("frame".into(), Value::from(frame)),
                    ("kind".into(), Value::from("snapshot_delta")),
                    ("changed".into(), Value::Object(delta.changed)),
                    (
                        "cleared".into(),
                        Value::Array(
                            delta.cleared.into_iter().map(Value::String).collect(),
                        ),
                    ),
                ]))
            }
        };
        self.prev = Some(sections);
        record
    }
}

impl Default for SnapshotDiffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Rebuild a snapshot-shaped object from sections (used for the full
/// frame and by tests).
pub fn flatten_sections(sections: &BTreeMap<&'static str, Value>) -> Value {
    Value::Object(
        sections
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_memory::uitree::UiNode;
    use eve_memory::{Address, Flavor};
    use eve_semantics::interaction::{InteractionElement, InteractionInfo};
    use eve_semantics::region::DisplayRegion;

    fn empty_tree() -> UiNode {
        UiNode {
            address: Address(1),
            type_name: "UIRoot".into(),
            entries: BTreeMap::new(),
            other_entries_keys: None,
            children: Some(vec![]),
        }
    }

    fn snapshot_with_node_count(count: usize) -> UiSnapshot {
        let mut s = eve_semantics::parse_ui_tree(&empty_tree(), Flavor::Infinity);
        s.node_count = count;
        s
    }

    fn interaction(address: u64, text: &str) -> InteractionElement {
        InteractionElement {
            type_name: "Button".into(),
            address: address.to_string(),
            name: None,
            role: None,
            label: None,
            text: Some(text.into()),
            hint: None,
            icon: None,
            icon_name: None,
            region: DisplayRegion { x: 0, y: 0, width: 10, height: 10 },
            visible_region: None,
            is_on_screen: true,
            interaction: InteractionInfo::default(),
        }
    }

    #[test]
    fn first_frame_is_full_then_empty_delta() {
        let mut differ = SnapshotDiffer::new();
        let line0 = differ.next_line(&snapshot_with_node_count(10));
        assert_eq!(line0["kind"], "snapshot_full");
        assert_eq!(line0["frame"], 0);
        assert_eq!(line0["snapshot"]["node_count"], 10);

        let line1 = differ.next_line(&snapshot_with_node_count(10));
        assert_eq!(line1["kind"], "snapshot_delta");
        assert_eq!(line1["changed"].as_object().unwrap().len(), 0);
        assert_eq!(line1["cleared"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn changed_section_reported() {
        let mut differ = SnapshotDiffer::new();
        differ.next_line(&snapshot_with_node_count(10));
        let line = differ.next_line(&snapshot_with_node_count(11));
        assert_eq!(line["changed"]["node_count"], 11);
    }

    #[test]
    fn keyed_diff_reports_add_update_remove_order() {
        let a = serde_json::to_value(interaction(100, "undock")).unwrap();
        let b = serde_json::to_value(interaction(200, "cargo")).unwrap();
        let b2 = serde_json::to_value(interaction(200, "cargo open")).unwrap();
        let prev = vec![a.clone(), b.clone()];
        let next = vec![b2.clone(), a.clone()];

        let d = keyed_diff(&prev, &next);
        assert_eq!(d["added"].as_array().unwrap().len(), 0);
        assert_eq!(d["removed"].as_array().unwrap().len(), 0);
        assert_eq!(d["updated"].as_array().unwrap().len(), 1);
        assert_eq!(d["updated"][0]["text"], "cargo open");
        // z-order flipped: 200 now on top
        assert_eq!(d["order"][0], "200");

        let d2 = keyed_diff(&prev, &[]);
        assert_eq!(d2["removed"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn interaction_elements_section_uses_keyed_form_in_delta() {
        let mut s0 = snapshot_with_node_count(1);
        s0.interaction_elements = vec![interaction(7, "x")];
        let mut s1 = snapshot_with_node_count(1);
        s1.interaction_elements = vec![interaction(7, "x"), interaction(8, "y")];

        let mut differ = SnapshotDiffer::new();
        differ.next_line(&s0);
        let line = differ.next_line(&s1);
        let keyed = &line["changed"][KEYED_SECTION];
        assert!(keyed.is_object());
        assert_eq!(keyed["added"].as_array().unwrap().len(), 1);
        assert_eq!(keyed["added"][0]["address"], "8");
    }
}
