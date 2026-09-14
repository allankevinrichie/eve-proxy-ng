//! CPython 2.7 (64-bit, Windows) object layout.
//!
//! Offsets cross-checked against the Sanderling reference implementation
//! (`implement/read-memory-64-bit/EveOnline64.cs`, BlindGuyNW fork 2026-07)
//! and the CPython 2.7 headers (`Include/object.h`, `stringobject.h`,
//! `dictobject.h`, `listobject.h`, `floatobject.h`):
//!
//! | Object | Field | Offset |
//! |---|---|---|
//! | PyObject | ob_refcnt | `0x00` |
//! | PyObject | ob_type | `0x08` |
//! | PyTypeObject | tp_name (char*) | `0x18` |
//! | PyTypeObject | tp_dictoffset | `0x120` |
//! | str | ob_size (chars) | `0x10` |
//! | str | ob_sval (inline bytes) | `0x20` |
//! | unicode (UCS-2) | length (chars) | `0x10` |
//! | unicode | buffer pointer | `0x18` |
//! | int / bool / float | value | `0x10` |
//! | dict | ma_mask | `0x20` |
//! | dict | ma_table (PyDictEntry*) | `0x28` |
//! | dict slot | { hash, key, value } | 3 × u64 |
//! | set | mask / table | `0x20` / `0x28` |
//! | set slot | { hash, key } | 2 × u64 |
//! | list / tuple | ob_size | `0x10` |
//! | list | ob_item (pointer array) | `0x18` |
//! | tuple | items (inline) | `0x18` |
//! | instance | `__dict__` pointer | `0x10` |

use std::collections::HashSet;

use rayon::prelude::*;

use crate::address::Address;
use crate::error::{Error, Result};
use crate::pyobject::PythonLayout;
use crate::source::{MemoryRegion, MemorySource};

/// Marker string CPython 2 uses for set tombstone entries.
const SET_DUMMY_KEY: &str = "<dummy key>";

/// CPython 2.7 object layout.
#[derive(Debug, Default, Clone, Copy)]
pub struct Py2Layout;

impl Py2Layout {
    pub fn new() -> Py2Layout {
        Py2Layout
    }
}

/// Read a whole region into memory. Returns an empty buffer when unreadable
/// (scans simply skip such regions, as partial data cannot be trusted).
fn read_region_bytes(mem: &dyn MemorySource, region: &MemoryRegion) -> Vec<u8> {
    let mut buffer = vec![0u8; region.size_bytes as usize];
    match mem.read_exact(region.base, &mut buffer) {
        Ok(()) => buffer,
        Err(e) => {
            tracing::debug!(region = %region.base, error = %e, "skipping unreadable region");
            Vec::new()
        }
    }
}

/// Interpret a byte buffer as little-endian u64 words.
pub(crate) fn as_words(bytes: &[u8]) -> impl Iterator<Item = u64> + '_ {
    bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))
}

fn read_nul_terminated_ascii(
    mem: &dyn MemorySource,
    string_address: Address,
    max_bytes: usize,
) -> Result<String> {
    if string_address.is_null() {
        return Err(Error::InvalidPyObject {
            address: 0,
            message: "null string pointer".into(),
        });
    }
    let mut buffer = vec![0u8; max_bytes];
    let received = mem.read(string_address, &mut buffer)?;
    let usable = &buffer[..received];
    let end = usable.iter().position(|&b| b == 0).unwrap_or(usable.len());
    // tp_name is ASCII in practice; lossy keeps scanning robust.
    Ok(String::from_utf8_lossy(&usable[..end]).into_owned())
}

impl PythonLayout for Py2Layout {
    fn name(&self) -> &'static str {
        "cpython-2.7-x64"
    }

    fn ob_type(&self, mem: &dyn MemorySource, object: Address) -> Result<Address> {
        let type_address = mem.read_u64(object + 0x8)?;
        if type_address == 0 {
            return Err(Error::InvalidPyObject {
                address: object.0,
                message: "null ob_type".into(),
            });
        }
        Ok(Address(type_address))
    }

    fn type_name(
        &self,
        mem: &dyn MemorySource,
        type_object: Address,
        max_bytes: usize,
    ) -> Result<String> {
        let string_address = mem.read_u64(type_object + 0x18)?;
        read_nul_terminated_ascii(mem, Address(string_address), max_bytes)
    }

    fn read_str(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> Result<String> {
        // Py2 PyStringObject keeps the characters inline right after the
        // 0x20 header, so one read covers both. A speculative read also
        // covers the overwhelmingly common case of size <= 240 bytes.
        const SPECULATIVE: usize = 0x20 + 240;
        let mut probe = [0u8; SPECULATIVE];
        if mem.read_exact(object, &mut probe).is_ok() {
            let size = i64::from_le_bytes(probe[0x10..0x18].try_into().unwrap());
            if (0..=240).contains(&size) {
                let take = (size as usize).min(max_chars);
                let mut text =
                    String::from_utf8_lossy(&probe[0x20..0x20 + take]).into_owned();
                if size as usize > max_chars {
                    text.push_str(" [...truncated]");
                }
                return Ok(text);
            }
            // Negative or large sizes fall through to the two-phase path.
            if size < 0 {
                return Err(Error::InvalidPyObject {
                    address: object.0,
                    message: "negative str ob_size".into(),
                });
            }
        }
        let mut header = [0u8; 0x20];
        mem.read_exact(object, &mut header)?;
        let size = i64::from_le_bytes(header[0x10..0x18].try_into().unwrap());
        if size == 0 {
            return Ok(String::new());
        }
        if size < 0 {
            return Err(Error::InvalidPyObject {
                address: object.0,
                message: "negative str ob_size".into(),
            });
        }
        let (chars_to_read, truncated) = if size as usize > max_chars {
            (max_chars, true)
        } else {
            (size as usize, false)
        };
        let mut bytes = vec![0u8; chars_to_read];
        mem.read_exact(object + 0x20, &mut bytes)?;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if truncated {
            text.push_str(" [...truncated]");
        }
        Ok(text)
    }

    fn read_unicode(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        max_chars: usize,
    ) -> Result<String> {
        let mut header = [0u8; 0x20];
        mem.read_exact(object, &mut header)?;
        let length = i64::from_le_bytes(header[0x10..0x18].try_into().unwrap());
        let buffer_address = u64::from_le_bytes(header[0x18..0x20].try_into().unwrap());
        if length == 0 {
            return Ok(String::new());
        }
        if length < 0 || buffer_address == 0 {
            return Err(Error::InvalidPyObject {
                address: object.0,
                message: "invalid unicode header".into(),
            });
        }
        let (chars_to_read, truncated) = if length as usize > max_chars {
            (max_chars, true)
        } else {
            (length as usize, false)
        };
        let mut bytes = vec![0u8; chars_to_read * 2];
        mem.read_exact(Address(buffer_address), &mut bytes)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let mut text = String::from_utf16_lossy(&units);
        if truncated {
            text.push_str(" [...truncated]");
        }
        Ok(text)
    }

    fn read_int(&self, mem: &dyn MemorySource, object: Address) -> Result<i64> {
        let mut header = [0u8; 0x18];
        mem.read_exact(object, &mut header)?;
        Ok(i64::from_le_bytes(header[0x10..0x18].try_into().unwrap()))
    }

    fn read_bool(&self, mem: &dyn MemorySource, object: Address) -> Result<bool> {
        Ok(self.read_int(mem, object)? != 0)
    }

    fn read_float(&self, mem: &dyn MemorySource, object: Address) -> Result<f64> {
        let mut header = [0u8; 0x20];
        mem.read_exact(object, &mut header)?;
        Ok(f64::from_le_bytes(header[0x10..0x18].try_into().unwrap()))
    }

    fn dict_entries(
        &self,
        mem: &dyn MemorySource,
        dict: Address,
        max_slots: usize,
    ) -> Result<Vec<(Address, Address)>> {
        let mut header = [0u8; 0x30];
        mem.read_exact(dict, &mut header)?;
        let mask = u64::from_le_bytes(header[0x20..0x28].try_into().unwrap());
        let table = u64::from_le_bytes(header[0x28..0x30].try_into().unwrap());
        if mask == u64::MAX || table == 0 {
            return Err(Error::InvalidPyObject {
                address: dict.0,
                message: "invalid dict header".into(),
            });
        }
        let slot_count = (mask + 1) as usize;
        if slot_count == 0 || slot_count > max_slots {
            return Err(Error::InvalidPyObject {
                address: dict.0,
                message: format!("dict too large: {slot_count} slots"),
            });
        }
        let mut table_bytes = vec![0u8; slot_count * 24];
        mem.read_exact(Address(table), &mut table_bytes)?;
        let mut entries = Vec::with_capacity(slot_count / 2);
        for slot in as_words(&table_bytes).collect::<Vec<_>>().chunks_exact(3) {
            let (_hash, key, value) = (slot[0], slot[1], slot[2]);
            if key == 0 || value == 0 {
                continue;
            }
            entries.push((Address(key), Address(value)));
        }
        Ok(entries)
    }

    fn set_keys(
        &self,
        mem: &dyn MemorySource,
        set: Address,
        max_slots: usize,
    ) -> Result<Vec<Address>> {
        let mut header = [0u8; 0x30];
        mem.read_exact(set, &mut header)?;
        let mask = u64::from_le_bytes(header[0x20..0x28].try_into().unwrap());
        let table = u64::from_le_bytes(header[0x28..0x30].try_into().unwrap());
        if mask == u64::MAX || table == 0 {
            return Err(Error::InvalidPyObject {
                address: set.0,
                message: "invalid set header".into(),
            });
        }
        let slot_count = (mask + 1) as usize;
        if slot_count == 0 || slot_count > max_slots {
            return Err(Error::InvalidPyObject {
                address: set.0,
                message: format!("set too large: {slot_count} slots"),
            });
        }
        let mut table_bytes = vec![0u8; slot_count * 16];
        mem.read_exact(Address(table), &mut table_bytes)?;
        let mut keys = Vec::new();
        for slot in as_words(&table_bytes).collect::<Vec<_>>().chunks_exact(2) {
            let key = slot[1];
            if key == 0 {
                continue;
            }
            // CPython 2 marks tombstones with a "<dummy key>" string object.
            if let Ok(text) = self.read_str(mem, Address(key), SET_DUMMY_KEY.len() + 1) {
                if text == SET_DUMMY_KEY {
                    continue;
                }
            }
            keys.push(Address(key));
        }
        Ok(keys)
    }

    fn sequence_len(&self, mem: &dyn MemorySource, object: Address) -> Result<i64> {
        let mut header = [0u8; 0x20];
        mem.read_exact(object, &mut header)?;
        let size = i64::from_le_bytes(header[0x10..0x18].try_into().unwrap());
        if size < 0 {
            return Err(Error::InvalidPyObject {
                address: object.0,
                message: "negative sequence length".into(),
            });
        }
        Ok(size)
    }

    fn list_items(
        &self,
        mem: &dyn MemorySource,
        list: Address,
        max_items: usize,
    ) -> Result<Vec<Address>> {
        let size = self.sequence_len(mem, list)?;
        if size as usize > max_items {
            return Err(Error::InvalidPyObject {
                address: list.0,
                message: format!("list too large: {size} items"),
            });
        }
        let mut header = [0u8; 0x20];
        mem.read_exact(list, &mut header)?;
        let items_pointer = u64::from_le_bytes(header[0x18..0x20].try_into().unwrap());
        if items_pointer == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = vec![0u8; (size * 8) as usize];
        mem.read_exact(Address(items_pointer), &mut bytes)?;
        Ok(as_words(&bytes).map(Address).collect())
    }

    fn tuple_items(
        &self,
        mem: &dyn MemorySource,
        tuple: Address,
        max_items: usize,
    ) -> Result<Vec<Address>> {
        let size = self.sequence_len(mem, tuple)?;
        if size as usize > max_items {
            return Err(Error::InvalidPyObject {
                address: tuple.0,
                message: format!("tuple too large: {size} items"),
            });
        }
        let mut bytes = vec![0u8; (size * 8) as usize];
        mem.read_exact(tuple + 0x18, &mut bytes)?;
        Ok(as_words(&bytes).map(Address).collect())
    }

    fn instance_dict_direct(
        &self,
        mem: &dyn MemorySource,
        object: Address,
    ) -> Result<Address> {
        Ok(Address(mem.read_u64(object + 0x10)?))
    }

    fn instance_dict_via_tp_dictoffset(
        &self,
        mem: &dyn MemorySource,
        object: Address,
        type_object: Address,
    ) -> Result<Option<Address>> {
        let mut type_bytes = [0u8; 8];
        mem.read_exact(type_object + 0x120, &mut type_bytes)?;
        let dict_offset = i64::from_le_bytes(type_bytes);
        if dict_offset <= 0 || dict_offset > 0x1000 {
            // Zero means "no instance dict"; a negative offset counts from
            // the end of a variable-length object and is not used here.
            return Ok(None);
        }
        Ok(Some(Address(mem.read_u64(object + dict_offset as u64)?)))
    }

    // ------------------------------------------------------------------
    // Compound reads (Py2 offsets; see the trait docs for semantics)
    // ------------------------------------------------------------------

    /// One 0x18 read: ob_type at +0x8, the common qword at +0x10.
    fn object_words(&self, mem: &dyn MemorySource, object: Address) -> Result<(Address, u64)> {
        let mut header = [0u8; 0x18];
        mem.read_exact(object, &mut header)?;
        let ob_type = u64::from_le_bytes(header[0x8..0x10].try_into().unwrap());
        let word = u64::from_le_bytes(header[0x10..0x18].try_into().unwrap());
        Ok((Address(ob_type), word))
    }

    /// One 0x20 read: ob_size at +0x10, ob_item at +0x18.
    fn list_words(&self, mem: &dyn MemorySource, list: Address) -> Result<Option<(i64, u64)>> {
        let mut header = [0u8; 0x20];
        mem.read_exact(list, &mut header)?;
        let len = i64::from_le_bytes(header[0x10..0x18].try_into().unwrap());
        let items = u64::from_le_bytes(header[0x18..0x20].try_into().unwrap());
        Ok(Some((len, items)))
    }

    // ------------------------------------------------------------------
    // Discovery scans
    // ------------------------------------------------------------------

    fn find_metatype_addresses(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
    ) -> HashSet<Address> {
        regions
            .par_iter()
            .filter_map(|region| {
                let bytes = read_region_bytes(mem, region);
                if bytes.len() < 0x28 {
                    return None;
                }
                let words: Vec<u64> = as_words(&bytes).collect();
                let mut found = Vec::new();
                // Candidate needs ob_type (word 1) plus tp_name (word 3).
                for i in 0..words.len().saturating_sub(3) {
                    let candidate = region.base + (i * 8) as u64;
                    // The metatype's ob_type points at the object itself.
                    if words[i + 1] != candidate.0 {
                        continue;
                    }
                    // tp_name pointer usually lives in a static region
                    // elsewhere; read it through the source, and skip the
                    // candidate (not the region) when unreadable.
                    let Ok(name_pointer) = mem.read_u64(candidate + 0x18) else {
                        continue;
                    };
                    if let Ok(name) =
                        read_nul_terminated_ascii(mem, Address(name_pointer), 256)
                    {
                        if name == "type" {
                            found.push(candidate);
                        }
                    }
                }
                Some(found)
            })
            .flatten()
            .collect()
    }

    fn find_type_objects(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        metatypes: &HashSet<Address>,
        max_name_bytes: usize,
    ) -> Vec<(Address, String)> {
        let (min, max) = address_bounds(metatypes);
        regions
            .par_iter()
            .filter_map(|region| {
                let bytes = read_region_bytes(mem, region);
                if bytes.len() < 0x28 {
                    return None;
                }
                let words: Vec<u64> = as_words(&bytes).collect();
                let mut found = Vec::new();
                for i in 0..words.len().saturating_sub(1) {
                    let type_pointer = words[i + 1];
                    if type_pointer < min || type_pointer > max {
                        continue;
                    }
                    if !metatypes.contains(&Address(type_pointer)) {
                        continue;
                    }
                    let candidate = region.base + (i * 8) as u64;
                    if let Ok(name_pointer) = mem.read_u64(candidate + 0x18) {
                        if let Ok(name) =
                            read_nul_terminated_ascii(mem, Address(name_pointer), max_name_bytes)
                        {
                            if !name.is_empty() {
                                found.push((candidate, name));
                            }
                        }
                    }
                }
                Some(found)
            })
            .flatten()
            .collect()
    }

    fn find_instances_of(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        types: &HashSet<Address>,
    ) -> Vec<Address> {
        let (min, max) = address_bounds(types);
        regions
            .par_iter()
            .filter_map(|region| {
                let bytes = read_region_bytes(mem, region);
                if bytes.len() < 0x10 {
                    return None;
                }
                let words: Vec<u64> = as_words(&bytes).collect();
                let mut found = Vec::new();
                for i in 0..words.len().saturating_sub(1) {
                    let type_pointer = words[i + 1];
                    if type_pointer < min || type_pointer > max {
                        continue;
                    }
                    if types.contains(&Address(type_pointer)) {
                        found.push(region.base + (i * 8) as u64);
                    }
                }
                Some(found)
            })
            .flatten()
            .collect()
    }

    fn count_instances_by_type(
        &self,
        mem: &dyn MemorySource,
        regions: &[MemoryRegion],
        types: &HashSet<Address>,
    ) -> std::collections::HashMap<Address, usize> {
        let (min, max) = address_bounds(types);
        regions
            .par_iter()
            .filter_map(|region| {
                let bytes = read_region_bytes(mem, region);
                if bytes.len() < 0x10 {
                    return None;
                }
                let words: Vec<u64> = as_words(&bytes).collect();
                let mut counts: std::collections::HashMap<Address, usize> =
                    std::collections::HashMap::new();
                for i in 0..words.len().saturating_sub(1) {
                    let type_pointer = words[i + 1];
                    if type_pointer < min || type_pointer > max {
                        continue;
                    }
                    if types.contains(&Address(type_pointer)) {
                        *counts.entry(Address(type_pointer)).or_insert(0) += 1;
                    }
                }
                Some(counts)
            })
            .flatten()
            .collect()
    }
}

/// Fast pre-filter bounds for membership checks during scans.
fn address_bounds(addresses: &HashSet<Address>) -> (u64, u64) {
    addresses
        .iter()
        .fold((u64::MAX, 0), |(min, max), a| (min.min(a.0), max.max(a.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryRegion;

    /// Synthetic memory source for layout unit tests.
    struct VecMemory {
        bytes: Vec<u8>,
    }

    impl VecMemory {
        fn u64(&mut self, offset: usize, value: u64) {
            self.bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }

    impl MemorySource for VecMemory {
        fn read(&self, address: Address, buffer: &mut [u8]) -> Result<usize> {
            let start = address.0 as usize;
            if start >= self.bytes.len() {
                return Err(Error::ReadFailed {
                    address: address.0,
                    message: "out of range".into(),
                });
            }
            let available = self.bytes.len() - start;
            let count = buffer.len().min(available);
            buffer[..count].copy_from_slice(&self.bytes[start..start + count]);
            Ok(count)
        }

        fn regions(&self) -> Result<Vec<MemoryRegion>> {
            Ok(vec![MemoryRegion {
                base: Address(0),
                size_bytes: self.bytes.len() as u64,
            }])
        }

        fn describe(&self) -> String {
            "test vector".into()
        }
    }

    #[test]
    fn decodes_str_object() {
        let mut mem = VecMemory {
            bytes: vec![0; 0x40],
        };
        mem.u64(0x08, 0x1000); // ob_type
        mem.u64(0x10, 5); // ob_size
        mem.bytes[0x20..0x25].copy_from_slice(b"hello");
        let layout = Py2Layout;
        assert_eq!(layout.read_str(&mem, Address(0), 4000).unwrap(), "hello");
    }

    #[test]
    fn truncates_long_str_with_marker() {
        let mut mem = VecMemory {
            bytes: vec![b'x'; 0x100],
        };
        mem.bytes[0x00..0x08].copy_from_slice(&[0; 8]);
        mem.u64(0x08, 0x1000);
        mem.u64(0x10, 0x100);
        let layout = Py2Layout;
        let text = layout.read_str(&mem, Address(0), 8).unwrap();
        assert!(text.starts_with("xxxxxxxx"));
        assert!(text.ends_with(" [...truncated]"));
    }

    #[test]
    fn decodes_dict_entries() {
        let mut mem = VecMemory { bytes: vec![0; 0x100] };
        let table = 0x80usize;
        mem.u64(0x08, 0x1000);
        mem.u64(0x20, 3); // ma_mask -> 4 slots
        mem.u64(0x28, table as u64);
        // slot 0: empty; slot 1: (key=1, value=2); slot 2: empty; slot 3: empty
        mem.u64(table + 24 + 8, 1);
        mem.u64(table + 24 + 16, 2);
        let layout = Py2Layout;
        let entries = layout.dict_entries(&mem, Address(0), 10_000).unwrap();
        assert_eq!(entries, vec![(Address(1), Address(2))]);
    }

    #[test]
    fn decodes_list_items() {
        let mut mem = VecMemory { bytes: vec![0; 0x100] };
        mem.u64(0x08, 0x1000);
        mem.u64(0x10, 3);
        mem.u64(0x18, 0x80);
        mem.u64(0x80, 11);
        mem.u64(0x88, 22);
        mem.u64(0x90, 33);
        let layout = Py2Layout;
        let items = layout.list_items(&mem, Address(0), 4000).unwrap();
        assert_eq!(items, vec![Address(11), Address(22), Address(33)]);
    }
}
