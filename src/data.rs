//! Caret - Memory-mapped dataset handling
//!
//! Provides zero-copy access to massive JSONL files using mmap.
//! Also supports Parquet and CSV formats via in-memory conversion.
//! Plus reading from stdin for pipeline workflows.

use anyhow::{Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::format::{self, InputFormat};

/// Storage backend for the dataset
enum DataStorage {
    /// Memory-mapped file (zero-copy, for large JSONL files)
    Mmap(Mmap),
    /// In-memory buffer (for stdin, Parquet, CSV, or small files)
    InMemory(Vec<u8>),
}

#[allow(dead_code)]
impl DataStorage {
    fn as_bytes(&self) -> &[u8] {
        match self {
            DataStorage::Mmap(m) => m.as_ref(),
            DataStorage::InMemory(v) => v.as_slice(),
        }
    }

    fn len(&self) -> usize {
        match self {
            DataStorage::Mmap(m) => m.len(),
            DataStorage::InMemory(v) => v.len(),
        }
    }
}

/// Dataset backed by memory-mapped file or in-memory buffer with pre-computed line offsets
pub struct Dataset {
    /// Data storage (mmap or in-memory)
    storage: DataStorage,
    /// Byte offsets for the start of each line
    line_offsets: Vec<usize>,
    /// File path for display
    pub path: String,
    /// File size in bytes
    pub size: u64,
    /// Original format (for display purposes)
    pub format: InputFormat,
}

impl Dataset {
    /// Open a file with automatic format detection
    ///
    /// Detects format from file extension and loads appropriately:
    /// - JSONL: Memory-mapped for instant O(1) line access
    /// - Parquet: Converted to JSONL in memory
    /// - CSV: Converted to JSONL in memory
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let format = InputFormat::detect(&path);
        Self::open_with_format(path, format)
    }

    /// Open a file with explicit format specification
    pub fn open_with_format<P: AsRef<Path>>(path: P, format: InputFormat) -> Result<Self> {
        let path_ref = path.as_ref();

        match format {
            InputFormat::Jsonl => Self::open_jsonl(path_ref),
            InputFormat::Parquet => Self::open_parquet(path_ref),
            InputFormat::Csv => Self::open_csv(path_ref),
        }
    }

    /// Open a JSONL file with memory mapping (zero-copy, instant open)
    fn open_jsonl(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open file: {}", path.display()))?;

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
            storage: DataStorage::Mmap(mmap),
            line_offsets,
            path: path.display().to_string(),
            size,
            format: InputFormat::Jsonl,
        })
    }

    /// Open a Parquet file (converts to JSONL in memory)
    fn open_parquet(path: &Path) -> Result<Self> {
        let lines = format::parquet_to_jsonl(path)?;
        Self::from_lines(lines, path.display().to_string(), InputFormat::Parquet)
    }

    /// Open a CSV file (converts to JSONL in memory)
    fn open_csv(path: &Path) -> Result<Self> {
        let lines = format::csv_to_jsonl(path)?;
        Self::from_lines(lines, path.display().to_string(), InputFormat::Csv)
    }

    /// Create a dataset from a vector of JSONL strings
    fn from_lines(lines: Vec<String>, path: String, format: InputFormat) -> Result<Self> {
        // Join lines with newlines
        let content = lines.join("\n");
        let buffer = content.into_bytes();
        let size = buffer.len() as u64;

        // Build line index
        let mut line_offsets = vec![0];
        for (i, &byte) in buffer.iter().enumerate() {
            if byte == b'\n' && i + 1 < buffer.len() {
                line_offsets.push(i + 1);
            }
        }

        Ok(Self {
            storage: DataStorage::InMemory(buffer),
            line_offsets,
            path,
            size,
            format,
        })
    }

    /// Read dataset from stdin
    ///
    /// Supports pipeline workflows: `cat data.jsonl | caret -`
    pub fn from_stdin() -> Result<Self> {
        let mut buffer = Vec::new();
        io::stdin().lock().read_to_end(&mut buffer)?;

        let size = buffer.len() as u64;

        // Build line index
        let mut line_offsets = vec![0];
        for (i, &byte) in buffer.iter().enumerate() {
            if byte == b'\n' && i + 1 < buffer.len() {
                line_offsets.push(i + 1);
            }
        }

        Ok(Self {
            storage: DataStorage::InMemory(buffer),
            line_offsets,
            path: "<stdin>".to_string(),
            size,
            format: InputFormat::Jsonl,
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

        let data = self.storage.as_bytes();
        let start = self.line_offsets[index];
        let end = if index + 1 < self.line_offsets.len() {
            self.line_offsets[index + 1] - 1 // Exclude newline
        } else {
            data.len()
        };

        // Handle edge case where line is empty or ends at file end
        let end = end.min(data.len());
        if start >= end {
            return Some("");
        }

        std::str::from_utf8(&data[start..end]).ok()
    }

    /// Get a range of lines for efficient batch access
    #[allow(dead_code)]
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

    /// Get format name for display
    pub fn format_name(&self) -> &'static str {
        match self.format {
            InputFormat::Jsonl => "JSONL",
            InputFormat::Parquet => "Parquet",
            InputFormat::Csv => "CSV",
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
        let mut file = NamedTempFile::with_suffix(".jsonl")?;
        writeln!(file, r#"{{"prompt": "Hello"}}"#)?;
        writeln!(file, r#"{{"prompt": "World"}}"#)?;
        writeln!(file, r#"{{"prompt": "Test"}}"#)?;

        let dataset = Dataset::open(file.path())?;
        assert_eq!(dataset.line_count(), 3);
        assert!(dataset.get_line(0).unwrap().contains("Hello"));
        assert!(dataset.get_line(1).unwrap().contains("World"));
        assert!(dataset.get_line(2).unwrap().contains("Test"));
        assert_eq!(dataset.format, InputFormat::Jsonl);
        Ok(())
    }

    #[test]
    fn test_csv_loading() -> Result<()> {
        let mut file = NamedTempFile::with_suffix(".csv")?;
        writeln!(file, "prompt,response")?;
        writeln!(file, "Hello,World")?;
        writeln!(file, "Foo,Bar")?;

        let dataset = Dataset::open(file.path())?;
        assert_eq!(dataset.line_count(), 2);
        assert!(dataset.get_line(0).unwrap().contains("Hello"));
        assert!(dataset.get_line(0).unwrap().contains("World"));
        assert_eq!(dataset.format, InputFormat::Csv);
        Ok(())
    }
}
