//! The Py2 compound-read overrides must be behaviorally identical to
//! the trait's default implementations (which future layouts without
//! overrides will use): walk the recorded sample with both and compare
//! the serialized trees byte-for-byte. Skipped without the sample.

use std::path::Path;

use eve_memory::pyobject::py2::Py2Layout;
use eve_memory::pyobject::PythonLayout;
use eve_memory::source::MemorySource;
use eve_memory::uitree::PersistentCaches;
use eve_memory::{Address, DumpSource, FrameReader, TreeLimits, TreeWalker};

const SAMPLE: &str = "../../samples/process-sample-a8b4bd7529.zip";

/// Delegates every primitive to `Py2Layout` but leaves the compound
/// reads (`object_words`, `list_words`) at their default composition —
/// the path a minimal Py3 layout would take.
struct DefaultsOnly;

impl PythonLayout for DefaultsOnly {
    fn name(&self) -> &'static str {
        Py2Layout.name()
    }
    fn ob_type(&self, mem: &dyn MemorySource, object: Address) -> eve_memory::Result<Address> {
        Py2Layout.ob_type(mem, object)
    }
    fn type_name(
        &self,
        mem: &dyn MemorySource,
        type_object: Address,
        max_bytes: usize,
    ) -> eve_memory::Result<String> {
        Py2Layout.type_name(mem, type_object, max_bytes)
    }
    fn read_str(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> eve_memory::Result<String> {
        Py2Layout.read_str(mem, object, max_chars)
    }
    fn read_unicode(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> eve_memory::Result<String> {
        Py2Layout.read_unicode(mem, object, max_chars)
    }
    fn read_int(&self, mem: &dyn MemorySource, object: Address) -> eve_memory::Result<i64> {
        Py2Layout.read_int(mem, object)
    }
    fn read_bool(&self, mem: &dyn MemorySource, object: Address) -> eve_memory::Result<bool> {
        Py2Layout.read_bool(mem, object)
    }
    fn read_float(&self, mem: &dyn MemorySource, object: Address) -> eve_memory::Result<f64> {
        Py2Layout.read_float(mem, object)
    }
    fn dict_entries(
        &self,
        mem: &dyn MemorySource,
        dict: Address,
        max_slots: usize,
    ) -> eve_memory::Result<Vec<(Address, Address)>> {
        Py2Layout.dict_entries(mem, dict, max_slots)
    }
    fn set_keys(
        &self,
        mem: &dyn MemorySource,
        set: Address,
        max_slots: usize,
    ) -> eve_memory::Result<Vec<Address>> {
        Py2Layout.set_keys(mem, set, max_slots)
    }
    fn sequence_len(&self, mem: &dyn MemorySource, object: Address) -> eve_memory::Result<i64> {
        Py2Layout.sequence_len(mem, object)
    }
    fn list_items(
        &self,
        mem: &dyn MemorySource,
        list: Address,
        max_items: usize,
    ) -> eve_memory::Result<Vec<Address>> {
        Py2Layout.list_items(mem, list, max_items)
    }
    fn tuple_items(
        &self,
        mem: &dyn MemorySource,
        tuple: Address,
        max_items: usize,
    ) -> eve_memory::Result<Vec<Address>> {
        Py2Layout.tuple_items(mem, tuple, max_items)
    }
    fn instance_dict_direct(
        &self,
        mem: &dyn MemorySource,
        object: Address,
    ) -> eve_memory::Result<Address> {
        Py2Layout.instance_dict_direct(mem, object)
    }
    fn instance_dict_via_tp_dictoffset(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        type_object: Address,
    ) -> eve_memory::Result<Option<Address>> {
        Py2Layout.instance_dict_via_tp_dictoffset(mem, object, type_object)
    }
    fn find_metatype_addresses(
        &self,
        mem: &dyn MemorySource,
        regions: &[eve_memory::MemoryRegion],
    ) -> std::collections::HashSet<Address> {
        Py2Layout.find_metatype_addresses(mem, regions)
    }
    fn find_type_objects(
        &self,
        mem: &dyn MemorySource,
        regions: &[eve_memory::MemoryRegion],
        metatypes: &std::collections::HashSet<Address>,
        max_name_bytes: usize,
    ) -> Vec<(Address, String)> {
        Py2Layout.find_type_objects(mem, regions, metatypes, max_name_bytes)
    }
    fn find_instances_of(
        &self,
        mem: &dyn MemorySource,
        regions: &[eve_memory::MemoryRegion],
        types: &std::collections::HashSet<Address>,
    ) -> Vec<Address> {
        Py2Layout.find_instances_of(mem, regions, types)
    }
    fn count_instances_by_type(
        &self,
        mem: &dyn MemorySource,
        regions: &[eve_memory::MemoryRegion],
        types: &std::collections::HashSet<Address>,
    ) -> std::collections::HashMap<Address, usize> {
        Py2Layout.count_instances_by_type(mem, regions, types)
    }
    // object_words / list_words intentionally NOT overridden: defaults.
}

#[test]
fn compound_reads_match_default_composition() {
    let sample = Path::new(SAMPLE);
    if !sample.exists() {
        eprintln!("skipping: {SAMPLE} not present");
        return;
    }
    let dump = DumpSource::open_zip(sample).expect("open sample");
    // Discover the root with the real layout once.
    let mut reader = eve_memory::UiReader::sample(sample).expect("reader");
    let root = reader.find_ui_root().expect("root");

    let caches_a = PersistentCaches::default();
    let frame_a = FrameReader::new(&dump);
    frame_a.set_parallel();
    let walker_a = TreeWalker::with_limits(
        &frame_a,
        &Py2Layout,
        TreeLimits::default(),
        &caches_a,
    );
    let tree_a = walker_a.read_tree_from(root).expect("walk A").expect("non-empty");

    let caches_b = PersistentCaches::default();
    let frame_b = FrameReader::new(&dump);
    frame_b.set_parallel();
    let walker_b =
        TreeWalker::with_limits(&frame_b, &DefaultsOnly, TreeLimits::default(), &caches_b);
    let tree_b = walker_b.read_tree_from(root).expect("walk B").expect("non-empty");

    let json_a = serde_json::to_vec(&tree_a).expect("serialize A");
    let json_b = serde_json::to_vec(&tree_b).expect("serialize B");
    assert_eq!(
        json_a, json_b,
        "compound overrides and default composition diverged"
    );
}
