//! LazyAlign - Memory-mapped dataset handling
//!
//! Provides zero-copy access to massive JSONL files using mmap.

use anyhow::{Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

/// Dataset backed by memory-mapped file with pre-computed line offsets
pub struct Dataset {
    /// Memory-mapped file content
    mmap: Mmap,
    /// Byte offsets for the start of each line
    line_offsets: Vec<usize>,
    /// File path for display
    pub path: String,
    /// File size in bytes
    pub size: u64,
}

impl Dataset {
    /// Open a file and build the line index
    ///
    /// This memory-maps the file (instant open) and scans for newlines
    /// to enable O(1) access to any line.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref)
            .with_context(|| format!("Failed to open file: {}", path_ref.display()))?;

        let metadata = file.metadata()?;
        let size = metadata.len();

        // Memory map the file - this is instant regardless of file size
        let mmap = unsafe { Mmap::map(&file)? };

        // Build line index by scanning for newlines
        let mut line_offsets = vec![0]; // First line starts at 0
        for (i, &byte) in mmap.iter().enumerate() {
            if byte == b'\n' && i + 1 < mmap.len() {
                line_offsets.push(i + 1);
            }
        }

        Ok(Self {
            mmap,
            line_offsets,
            path: path_ref.display().to_string(),
            size,
        })
    }

    /// Get the total number of lines in the file
    pub fn line_count(&self) -> usize {
        self.line_offsets.len()
    }

    /// Get a specific line by index (0-indexed)
    ///
    /// Returns None if index is out of bounds.
    /// This is O(1) thanks to the pre-computed line offsets.
    pub fn get_line(&self, index: usize) -> Option<&str> {
        if index >= self.line_offsets.len() {
            return None;
        }

        let start = self.line_offsets[index];
        let end = if index + 1 < self.line_offsets.len() {
            self.line_offsets[index + 1] - 1 // Exclude newline
        } else {
            self.mmap.len()
        };

        // Handle edge case where line is empty or ends at file end
        let end = end.min(self.mmap.len());
        if start >= end {
            return Some("");
        }

        std::str::from_utf8(&self.mmap[start..end]).ok()
    }

    /// Get a range of lines for efficient batch access
    pub fn get_lines(&self, start: usize, count: usize) -> Vec<&str> {
        (start..start + count)
            .filter_map(|i| self.get_line(i))
            .collect()
    }

    /// Get formatted file size string
    pub fn size_human(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if self.size >= GB {
            format!("{:.2} GB", self.size as f64 / GB as f64)
        } else if self.size >= MB {
            format!("{:.2} MB", self.size as f64 / MB as f64)
        } else if self.size >= KB {
            format!("{:.2} KB", self.size as f64 / KB as f64)
        } else {
            format!("{} B", self.size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_line_access() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        writeln!(file, r#"{{"prompt": "Hello"}}"#)?;
        writeln!(file, r#"{{"prompt": "World"}}"#)?;
        writeln!(file, r#"{{"prompt": "Test"}}"#)?;

        let dataset = Dataset::open(file.path())?;
        assert_eq!(dataset.line_count(), 3);
        assert!(dataset.get_line(0).unwrap().contains("Hello"));
        assert!(dataset.get_line(1).unwrap().contains("World"));
        assert!(dataset.get_line(2).unwrap().contains("Test"));
        Ok(())
    }
}
