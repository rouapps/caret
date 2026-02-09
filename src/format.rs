//! Caret - Multi-format input support
//!
//! Detects and converts various dataset formats (JSONL, Parquet, CSV) to a common representation.

use anyhow::{Context, Result};
use arrow::json::LineDelimitedWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Supported input formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    /// JSON Lines format (one JSON object per line)
    Jsonl,
    /// Apache Parquet columnar format
    Parquet,
    /// Comma-separated values with header row
    Csv,
}

impl InputFormat {
    /// Detect format from file extension
    pub fn detect<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        match path.extension().and_then(|e| e.to_str()) {
            Some("parquet") | Some("pq") => InputFormat::Parquet,
            Some("csv") => InputFormat::Csv,
            Some("tsv") => InputFormat::Csv, // TSV is similar enough
            _ => InputFormat::Jsonl, // Default to JSONL
        }
    }

    /// Parse format from string (for CLI)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "jsonl" | "json" | "ndjson" => Some(InputFormat::Jsonl),
            "parquet" | "pq" => Some(InputFormat::Parquet),
            "csv" => Some(InputFormat::Csv),
            "auto" => None, // Caller should use detect()
            _ => None,
        }
    }
}

/// Convert a Parquet file to JSONL strings in memory
pub fn parquet_to_jsonl<P: AsRef<Path>>(path: P) -> Result<Vec<String>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("Failed to open Parquet file: {}", path.display()))?;

    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| "Failed to read Parquet metadata")?;

    let reader = builder.build()
        .with_context(|| "Failed to build Parquet reader")?;

    let mut lines = Vec::new();

    for batch_result in reader {
        let batch = batch_result.with_context(|| "Failed to read Parquet batch")?;
        
        // Convert batch to JSON using Arrow's JSON writer
        let mut buf = Vec::new();
        {
            let mut writer = LineDelimitedWriter::new(&mut buf);
            writer.write(&batch).with_context(|| "Failed to serialize batch to JSON")?;
            writer.finish().with_context(|| "Failed to finish JSON writer")?;
        }

        // Split into lines
        let json_str = String::from_utf8(buf)
            .with_context(|| "Invalid UTF-8 in JSON output")?;
        
        for line in json_str.lines() {
            if !line.trim().is_empty() {
                lines.push(line.to_string());
            }
        }
    }

    Ok(lines)
}

/// Convert a CSV file to JSONL strings in memory
pub fn csv_to_jsonl<P: AsRef<Path>>(path: P) -> Result<Vec<String>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("Failed to open CSV file: {}", path.display()))?;

    let mut reader = csv::Reader::from_reader(BufReader::new(file));
    let headers: Vec<String> = reader.headers()
        .with_context(|| "Failed to read CSV headers")?
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut lines = Vec::new();

    for result in reader.records() {
        let record = result.with_context(|| "Failed to read CSV record")?;
        
        // Build JSON object from headers and values
        let mut obj = serde_json::Map::new();
        for (header, value) in headers.iter().zip(record.iter()) {
            obj.insert(header.clone(), serde_json::Value::String(value.to_string()));
        }
        
        let json_line = serde_json::to_string(&serde_json::Value::Object(obj))
            .with_context(|| "Failed to serialize CSV row to JSON")?;
        lines.push(json_line);
    }

    Ok(lines)
}

/// Read a JSONL file and return lines as strings
pub fn read_jsonl_lines<P: AsRef<Path>>(path: P) -> Result<Vec<String>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("Failed to open JSONL file: {}", path.display()))?;

    let reader = BufReader::new(file);
    let mut lines = Vec::new();

    for line in reader.lines() {
        let line = line.with_context(|| "Failed to read line")?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format() {
        assert_eq!(InputFormat::detect("data.jsonl"), InputFormat::Jsonl);
        assert_eq!(InputFormat::detect("data.parquet"), InputFormat::Parquet);
        assert_eq!(InputFormat::detect("data.pq"), InputFormat::Parquet);
        assert_eq!(InputFormat::detect("data.csv"), InputFormat::Csv);
        assert_eq!(InputFormat::detect("data.txt"), InputFormat::Jsonl); // Default
        assert_eq!(InputFormat::detect("data"), InputFormat::Jsonl); // No extension
    }

    #[test]
    fn test_parse() {
        assert_eq!(InputFormat::parse("jsonl"), Some(InputFormat::Jsonl));
        assert_eq!(InputFormat::parse("PARQUET"), Some(InputFormat::Parquet));
        assert_eq!(InputFormat::parse("csv"), Some(InputFormat::Csv));
        assert_eq!(InputFormat::parse("auto"), None);
        assert_eq!(InputFormat::parse("unknown"), None);
    }
}
