//! Client resource cache: read `res:/` files from the game's shared
//! cache on disk.
//!
//! The launcher materializes game resources as content-addressed files:
//! `resfileindex.txt` maps each `res:/…` path to a
//! `<2-hex>/<sha1>_<sha1>` file under `ResFiles/`. Icon PNGs, textures,
//! and other assets are therefore fully available offline — no memory
//! reading involved.
//!
//! Layout (曙光 install 2026-09):
//! ```text
//! C:\EVE\SharedCache\
//! ├── ResFiles\8e\8eb844…_2aae…        ← content files (hashed names)
//! └── infinity\resfileindex.txt         ← res path → hashed file
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Read-only view over the shared cache's resource files.
pub struct ResourceCache {
    root: PathBuf,
    /// normalized res path → relative hashed file path.
    index: HashMap<String, String>,
}

impl ResourceCache {
    /// Open the cache at the shared-cache root (the directory containing
    /// `ResFiles/`, e.g. `C:\EVE\SharedCache`). Uses
    /// `<root>/<flavor>/resfileindex.txt` — pass the flavor dir name
    /// (e.g. `infinity`) when several are installed.
    pub fn open(shared_cache_root: &Path, flavor: &str) -> Result<ResourceCache> {
        let index_path = shared_cache_root.join(flavor).join("resfileindex.txt");
        let text = std::fs::read_to_string(&index_path).map_err(|e| Error::Other(format!(
            "reading {}: {e}",
            index_path.display()
        )))?;
        let mut index = HashMap::with_capacity(80_000);
        for line in text.lines() {
            // respath,relpath,sha1,origsize,size
            let mut columns = line.split(',');
            let (Some(res_path), Some(relative)) = (columns.next(), columns.next()) else {
                continue;
            };
            if !res_path.starts_with("res:/") || relative.is_empty() {
                continue;
            }
            index.insert(normalize(res_path), relative.to_string());
        }
        if index.is_empty() {
            return Err(Error::Other(format!(
                "no entries in {}",
                index_path.display()
            )));
        }
        Ok(ResourceCache {
            root: shared_cache_root.to_path_buf(),
            index,
        })
    }

    /// The shared-cache root this cache was opened from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Read one resource file (`res:/ui/texture/icons/13_64_5.png`).
    pub fn read(&self, res_path: &str) -> Result<Vec<u8>> {
        let relative = self
            .index
            .get(&normalize(res_path))
            .ok_or_else(|| Error::Other(format!("resource not indexed: {res_path}")))?;
        std::fs::read(self.root.join("ResFiles").join(relative)).map_err(|e| {
            Error::Other(format!("reading {res_path} ({relative}): {e}"))
        })
    }

    /// Derive the shared-cache root from a running client's executable
    /// path (`…\SharedCache\<flavor>\bin64\exefile.exe`).
    pub fn root_from_client_exe(exe_path: &Path) -> Option<PathBuf> {
        // exe → bin64 → <flavor> → SharedCache
        exe_path.ancestors().nth(3).map(Path::to_path_buf)
    }
}

fn normalize(res_path: &str) -> String {
    res_path.trim().replace('\\', "/").to_ascii_lowercase()
}
