//! NDJSON sink with periodic flush: crash-safe (line-granular) without a
//! syscall per line.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use serde_json::Value;

const FLUSH_INTERVAL_MS: u64 = 200;

pub struct JsonlWriter {
    writer: BufWriter<File>,
    last_flush: Instant,
    pub lines: u64,
}

impl JsonlWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            writer: BufWriter::new(File::create(path)?),
            last_flush: Instant::now(),
            lines: 0,
        })
    }

    pub fn write(&mut self, value: &Value) -> std::io::Result<()> {
        // A line of NDJSON never contains raw newlines (serde_json escapes
        // them), so one `write_all` + newline keeps the file parseable at
        // any interruption point after flush.
        serde_json::to_writer(&mut self.writer, value)?;
        self.writer.write_all(b"\n")?;
        self.lines += 1;
        if self.last_flush.elapsed().as_millis() as u64 >= FLUSH_INTERVAL_MS {
            self.writer.flush()?;
            self.last_flush = Instant::now();
        }
        Ok(())
    }

    pub fn finish(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
