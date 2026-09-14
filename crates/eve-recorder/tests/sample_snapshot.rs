//! Offline integration: drive the delta encoder from the recorded
//! Sanderling-compatible sample (no game required). Skipped when the
//! sample archive is not present (samples are not checked in).

use std::path::Path;

use eve_memory::{Flavor, UiReader};
use eve_recorder::delta::SnapshotDiffer;
use eve_semantics::parse_ui_tree;

const SAMPLE: &str = "../../samples/process-sample-a8b4bd7529.zip";

#[test]
fn snapshot_full_then_empty_delta_from_sample() {
    let sample = Path::new(SAMPLE);
    if !sample.exists() {
        eprintln!("skipping: {SAMPLE} not present");
        return;
    }
    let mut reader = UiReader::sample(sample).expect("open sample");
    let tree = reader.read_tree().expect("read tree");
    let snap = parse_ui_tree(&tree, Flavor::Infinity);

    let mut differ = SnapshotDiffer::new();
    let full = differ.next_line(&snap);
    assert_eq!(full["kind"], "snapshot_full");
    assert_eq!(full["frame"], 0);
    // The full frame's keyed section matches the parsed snapshot.
    let elements = snap.interaction_elements.len();
    assert_eq!(
        full["snapshot"]["interaction_elements"].as_array().map(|a| a.len()),
        Some(elements)
    );
    // Every element carries its address (the keyed-diff identity).
    for element in full["snapshot"]["interaction_elements"].as_array().unwrap() {
        assert!(element["address"].is_string(), "element missing address");
    }
    // A second identical frame is a well-formed empty delta.
    let delta = differ.next_line(&snap);
    assert_eq!(delta["kind"], "snapshot_delta");
    assert_eq!(delta["frame"], 1);
    assert_eq!(delta["changed"].as_object().unwrap().len(), 0);
    assert_eq!(delta["cleared"].as_array().unwrap().len(), 0);

    // Mutating one interaction element produces a keyed diff, not a full
    // section rewrite.
    let mut changed = snap.clone();
    if let Some(first) = changed.interaction_elements.first_mut() {
        first.text = Some("__mutated__".into());
    }
    let delta2 = differ.next_line(&changed);
    let keyed = &delta2["changed"]["interaction_elements"];
    assert_eq!(keyed["updated"].as_array().unwrap().len(), 1);
    assert_eq!(keyed["added"].as_array().unwrap().len(), 0);
    assert_eq!(keyed["removed"].as_array().unwrap().len(), 0);
}
