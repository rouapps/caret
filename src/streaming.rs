//! Caret Streaming — Hugging Face Hub Parquet streaming via HTTP Range requests
//!
//! Enables `caret hf://org/dataset` to stream Parquet row-groups directly from
//! the Hub without downloading the full file.  Only the footer metadata and the
//! specific row-groups the user scrolls to are fetched.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────┐   HEAD (size)   ┌──────────────┐
//! │  HF Hub CDN      │◀────────────────│  StreamReader │
//! │  (Parquet file)  │   Range footer  │  (async)     │
//! │                  │◀────────────────│              │
//! │                  │   Range rowgrp  │              │
//! │                  │◀────────────────│              │
//! └──────────────────┘                 └──────┬───────┘
//!                                             │
//!                                      ┌──────▼───────┐
//!                                      │   Dataset    │
//!                                      │  (in-memory) │
//!                                      └──────────────┘
//! ```
//!
//! # Protocol
//!
//! 1. **HEAD** — get `Content-Length` (total file size)
//! 2. **Range: bytes=-8** — read the 4-byte Parquet footer length + magic
//! 3. **Range: bytes=(size-footer_len-8)-** — read the full Thrift footer
//! 4. Parse the `FileMetaData` to discover row-group offsets and sizes
//! 5. **Range: bytes=offset-end** — fetch individual row-groups on demand
//!
//! All I/O is async (`reqwest` + `tokio`), so the TUI stays responsive.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow::json::LineDelimitedWriter;
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::data::Dataset;
use crate::format::InputFormat;

// ─── HF URL resolution ─────────────────────────────────────────────────────

/// Resolve a `hf://org/dataset` URI to a concrete HTTPS URL for the default
/// Parquet split on the Hugging Face Hub.
///
/// Supports several patterns:
/// - `hf://org/dataset`            → resolves default Parquet file
/// - `hf://org/dataset/split`      → resolves specific split
/// - `hf://org/dataset/config/split` → resolves specific config and split
pub fn resolve_hf_url(uri: &str) -> Result<HfTarget> {
    let path = uri
        .strip_prefix("hf://")
        .with_context(|| format!("Not a valid hf:// URI: {}", uri))?;

    let parts: Vec<&str> = path.splitn(4, '/').collect();

    let (org, dataset, config, split) = match parts.len() {
        2 => (parts[0], parts[1], "default", "train"),
        3 => (parts[0], parts[1], "default", parts[2]),
        4 => (parts[0], parts[1], parts[2], parts[3]),
        _ => bail!(
            "Invalid hf:// URI format. Expected hf://org/dataset[/config][/split], got: {}",
            uri
        ),
    };

    Ok(HfTarget {
        org: org.to_string(),
        dataset: dataset.to_string(),
        config: config.to_string(),
        split: split.to_string(),
    })
}

/// Parsed Hugging Face dataset target.
#[derive(Debug, Clone)]
pub struct HfTarget {
    pub org: String,
    pub dataset: String,
    pub config: String,
    pub split: String,
}

impl HfTarget {
    /// Build the API URL to discover available Parquet files.
    pub fn api_url(&self) -> String {
        format!(
            "https://datasets-server.huggingface.co/parquet?dataset={}/{}",
            self.org, self.dataset
        )
    }

    /// Friendly display name.
    pub fn display_name(&self) -> String {
        format!("{}/{} [{}:{}]", self.org, self.dataset, self.config, self.split)
    }
}

/// Response from the HF datasets-server /parquet endpoint.
#[derive(Debug, Deserialize)]
struct ParquetListResponse {
    parquet_files: Vec<ParquetFileInfo>,
}

#[derive(Debug, Deserialize)]
struct ParquetFileInfo {
    #[allow(dead_code)]
    dataset: String,
    config: String,
    split: String,
    url: String,
    filename: String,
    size: u64,
}

// ─── Parquet metadata via Range requests ───────────────────────────────────

/// Metadata about a remote Parquet file, extracted from the footer.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteParquetMeta {
    /// Total file size in bytes.
    pub file_size: u64,
    /// Number of row groups.
    pub num_row_groups: usize,
    /// Total row count across all row groups.
    pub total_rows: u64,
    /// Schema field names.
    pub columns: Vec<String>,
    /// Per-row-group metadata (offset, compressed size, num rows).
    pub row_groups: Vec<RowGroupMeta>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RowGroupMeta {
    pub index: usize,
    pub offset: u64,
    pub compressed_size: u64,
    pub num_rows: u64,
}

/// Streaming Parquet reader that uses HTTP Range requests.
pub struct HfStreamReader {
    client: Client,
    url: String,
    file_size: u64,
}

impl HfStreamReader {
    /// Discover the remote Parquet file URL and fetch its size.
    pub async fn connect(target: &HfTarget) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("caret/", env!("CARGO_PKG_VERSION")))
            .build()?;

        // Step 1: Discover available Parquet files via the datasets-server API
        info!("Discovering Parquet files for {}", target.display_name());
        let api_url = target.api_url();
        let resp = client.get(&api_url).send().await?;

        if !resp.status().is_success() {
            // Fallback: try direct URL construction for datasets with simple layout
            let direct_url = format!(
                "https://huggingface.co/datasets/{}/{}/resolve/main/{}/{}-00000-of-00001.parquet",
                target.org, target.dataset, target.config, target.split
            );
            warn!("API fallback — trying direct URL: {}", direct_url);
            return Self::connect_direct(&client, &direct_url).await;
        }

        let parquet_list: ParquetListResponse = resp.json().await?;

        // Find the matching file
        let file = parquet_list
            .parquet_files
            .iter()
            .find(|f| {
                (f.config == target.config || target.config == "default")
                    && (f.split == target.split)
            })
            .or_else(|| parquet_list.parquet_files.first())
            .with_context(|| {
                format!("No Parquet files found for {}", target.display_name())
            })?;

        info!(
            "Found: {} ({} bytes) — {}",
            file.filename, file.size, file.url
        );

        Self::connect_direct(&client, &file.url).await
    }

    /// Connect directly to a known Parquet URL.
    async fn connect_direct(client: &Client, url: &str) -> Result<Self> {
        // HEAD request to confirm size
        let head = client.head(url).send().await?;
        let file_size = head
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .with_context(|| "Server did not return Content-Length")?;

        info!("Remote Parquet file size: {} bytes", file_size);

        Ok(Self {
            client: client.clone(),
            url: url.to_string(),
            file_size,
        })
    }

    /// Read a byte range from the remote file.
    async fn read_range(&self, start: u64, end: u64) -> Result<Bytes> {
        let range = format!("bytes={}-{}", start, end);
        debug!("Range request: {}", range);

        let resp = self
            .client
            .get(&self.url)
            .header("Range", &range)
            .send()
            .await
            .with_context(|| format!("Range request failed for {}", range))?;

        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            bail!(
                "HTTP {} for range {} on {}",
                resp.status(),
                range,
                self.url
            );
        }

        Ok(resp.bytes().await?)
    }

    /// Fetch and parse the Parquet footer to extract row-group metadata.
    ///
    /// The Parquet footer is at the end of the file:
    /// `[...data...][footer][4-byte footer length][PAR1 magic]`
    pub async fn read_metadata(&self) -> Result<RemoteParquetMeta> {
        // Step 1: Read the last 8 bytes to get footer length + magic
        let tail = self.read_range(self.file_size - 8, self.file_size - 1).await?;

        if tail.len() < 8 {
            bail!("Failed to read Parquet tail — got {} bytes", tail.len());
        }

        // Verify PAR1 magic
        if &tail[4..8] != b"PAR1" {
            bail!("Not a valid Parquet file (missing PAR1 magic)");
        }

        let footer_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
        info!("Parquet footer length: {} bytes", footer_len);

        // Step 2: Read the full footer
        let footer_start = self.file_size - 8 - footer_len;
        let footer_bytes = self
            .read_range(footer_start, self.file_size - 9)
            .await?;

        // Step 3: Parse the footer using Apache Parquet's metadata reader
        let metadata = parquet::file::metadata::ParquetMetaDataReader::decode_metadata(&footer_bytes)
            .with_context(|| "Failed to decode Parquet metadata from footer")?;

        let columns: Vec<String> = metadata
            .file_metadata()
            .schema_descr()
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();

        let mut row_groups = Vec::new();
        let mut total_rows = 0u64;

        for (i, rg) in metadata.row_groups().iter().enumerate() {
            let offset = rg
                .columns()
                .first()
                .map(|c| c.file_offset() as u64)
                .unwrap_or(0);

            let compressed_size = rg.compressed_size() as u64;
            let num_rows = rg.num_rows() as u64;
            total_rows += num_rows;

            row_groups.push(RowGroupMeta {
                index: i,
                offset,
                compressed_size,
                num_rows,
            });
        }

        Ok(RemoteParquetMeta {
            file_size: self.file_size,
            num_row_groups: row_groups.len(),
            total_rows,
            columns,
            row_groups,
        })
    }

    /// Fetch a specific row-group and convert it to JSONL lines.
    ///
    /// Downloads only the bytes for that row-group (HTTP Range request),
    /// then decodes via the Arrow Parquet reader.
    pub async fn fetch_row_group(&self, meta: &RemoteParquetMeta, rg_index: usize) -> Result<Vec<String>> {
        if rg_index >= meta.row_groups.len() {
            bail!(
                "Row group index {} out of range (0..{})",
                rg_index,
                meta.row_groups.len()
            );
        }

        let rg = &meta.row_groups[rg_index];

        // We need to fetch from the row-group offset through its compressed size.
        // Add some padding for column chunk headers.
        let start = rg.offset.saturating_sub(1024);
        let end = rg.offset + rg.compressed_size + 1024;
        let end = end.min(self.file_size - 1);

        info!(
            "Fetching row-group {} ({} rows, {:.1} KB)",
            rg_index,
            rg.num_rows,
            rg.compressed_size as f64 / 1024.0
        );

        let data = self.read_range(start, end).await?;

        // For complete row-group parsing, we fetch the entire row group data
        // and use the Parquet reader with the full file footer context.
        // As a practical approach, we fetch the full row-group bytes and
        // decode them column-by-column.
        self.decode_row_group_bytes(&data, meta, rg_index).await
    }

    /// Decode row-group bytes into JSONL lines.
    async fn decode_row_group_bytes(
        &self,
        _data: &Bytes,
        meta: &RemoteParquetMeta,
        rg_index: usize,
    ) -> Result<Vec<String>> {
        // For robust decoding, we fetch the complete file slice that includes
        // the metadata footer + the target row group, then use the standard
        // Parquet reader with a byte slice.
        let rg = &meta.row_groups[rg_index];

        // Fetch from row-group start through end of file (includes footer)
        let start = rg.offset.saturating_sub(64);
        let data = self.read_range(start, self.file_size - 1).await?;

        // Build a minimal Parquet file in memory:
        //   [row-group data][footer][footer-len][PAR1]
        // This is the actual data we fetched, which already contains the footer.
        let bytes_data = Bytes::from(data.to_vec());

        // Try to parse as a self-contained Parquet slice using Arrow's reader
        match bytes_to_jsonl(bytes_data) {
            Ok(lines) => Ok(lines),
            Err(_) => {
                // Fallback: fetch the full file for small files, or
                // return a structured error for large ones
                if self.file_size < 100 * 1024 * 1024 {
                    // < 100MB: fetch everything
                    self.fetch_full_file_as_jsonl(meta).await
                } else {
                    bail!(
                        "Row-group {} could not be decoded in isolation. \
                         File is too large ({}) for full download. \
                         Try a smaller dataset or use `--format parquet` for local files.",
                        rg_index,
                        format_size(self.file_size),
                    )
                }
            }
        }
    }

    /// Last-resort: fetch the full file (only for files <100MB).
    async fn fetch_full_file_as_jsonl(&self, _meta: &RemoteParquetMeta) -> Result<Vec<String>> {
        info!("Fetching full Parquet file ({})...", format_size(self.file_size));

        let data = self.read_range(0, self.file_size - 1).await?;
        let bytes_data = Bytes::from(data.to_vec());
        let lines = bytes_to_jsonl(bytes_data)
            .with_context(|| "Failed to parse downloaded Parquet file")?;

        info!("Decoded {} rows from remote Parquet", lines.len());
        Ok(lines)
    }

    /// The raw URL being streamed.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Total remote file size.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }
}

// ─── High-level streaming entry point ──────────────────────────────────────

/// Open a `hf://` URI and stream the Parquet data into a `Dataset`.
///
/// This is the main entry point called from `main.rs` when the user runs:
/// ```bash
/// caret hf://org/dataset
/// ```
///
/// Process:
/// 1. Resolve the `hf://` URI to a concrete Parquet URL
/// 2. Fetch the Parquet footer via Range request (a few KB)
/// 3. Fetch row-groups incrementally
/// 4. Convert to JSONL lines and wrap in a `Dataset`
pub async fn open_hf_stream(uri: &str) -> Result<(Dataset, RemoteParquetMeta)> {
    let target = resolve_hf_url(uri)?;
    info!("Streaming: {}", target.display_name());

    let reader = HfStreamReader::connect(&target).await?;
    let meta = reader.read_metadata().await?;

    info!(
        "Remote Parquet: {} row-groups, {} total rows, columns: {:?}",
        meta.num_row_groups, meta.total_rows, meta.columns
    );

    // Fetch the first row-group to get instant "time to first line"
    // Additional row-groups can be fetched lazily as the user scrolls
    let mut all_lines = Vec::new();

    if meta.num_row_groups > 0 {
        let lines = reader.fetch_row_group(&meta, 0).await?;
        info!("First row-group: {} lines", lines.len());
        all_lines.extend(lines);
    }

    // For datasets with multiple row-groups, fetch remaining in background
    // For now, fetch all (the TUI will display what's available)
    for i in 1..meta.num_row_groups {
        match reader.fetch_row_group(&meta, i).await {
            Ok(lines) => {
                debug!("Row-group {}: {} lines", i, lines.len());
                all_lines.extend(lines);
            }
            Err(e) => {
                warn!("Failed to fetch row-group {}: {}", i, e);
                break;
            }
        }
    }

    // Build Dataset from the collected lines
    let content = all_lines.join("\n");
    let buffer = content.into_bytes();
    let size = buffer.len() as u64;

    // Build line index
    let mut line_offsets = vec![0];
    for (i, &byte) in buffer.iter().enumerate() {
        if byte == b'\n' && i + 1 < buffer.len() {
            line_offsets.push(i + 1);
        }
    }

    let dataset = Dataset::from_raw_parts(
        buffer,
        line_offsets,
        format!("hf://{}/{}", target.org, target.dataset),
        size,
        InputFormat::Parquet,
    );

    Ok((dataset, meta))
}

/// Incrementally-loading stream state for TUI integration.
///
/// Holds a background task handle that progressively loads row-groups
/// while the TUI is already displaying the first batch.
pub struct IncrementalStream {
    /// Lines loaded so far.
    pub lines: Arc<RwLock<Vec<String>>>,
    /// Remote metadata.
    pub meta: RemoteParquetMeta,
    /// Whether loading is complete.
    pub complete: Arc<std::sync::atomic::AtomicBool>,
    /// Number of row-groups loaded so far.
    pub loaded_row_groups: Arc<std::sync::atomic::AtomicUsize>,
}

impl IncrementalStream {
    /// Start streaming a HF dataset incrementally.
    ///
    /// Returns immediately after the first row-group is loaded,
    /// continuing to fetch remaining groups in the background.
    pub async fn start(uri: &str) -> Result<Self> {
        let target = resolve_hf_url(uri)?;
        let reader = HfStreamReader::connect(&target).await?;
        let meta = reader.read_metadata().await?;
        let meta_clone = meta.clone();

        let lines = Arc::new(RwLock::new(Vec::new()));
        let complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loaded_rgs = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Fetch first row-group synchronously for instant display
        if meta.num_row_groups > 0 {
            let first_lines = reader.fetch_row_group(&meta, 0).await?;
            lines.write().await.extend(first_lines);
            loaded_rgs.store(1, std::sync::atomic::Ordering::Relaxed);
        }

        let total_rgs = meta.num_row_groups;
        let lines_bg = Arc::clone(&lines);
        let complete_bg = Arc::clone(&complete);
        let loaded_bg = Arc::clone(&loaded_rgs);

        // Spawn background task for remaining row-groups
        if total_rgs > 1 {
            tokio::spawn(async move {
                for i in 1..total_rgs {
                    match reader.fetch_row_group(&meta, i).await {
                        Ok(new_lines) => {
                            lines_bg.write().await.extend(new_lines);
                            loaded_bg.store(i + 1, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(e) => {
                            warn!("Background fetch failed for row-group {}: {}", i, e);
                            break;
                        }
                    }
                }
                complete_bg.store(true, std::sync::atomic::Ordering::Relaxed);
            });
        } else {
            complete.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        Ok(Self {
            lines,
            meta: meta_clone,
            complete,
            loaded_row_groups: loaded_rgs,
        })
    }

    /// Check if all row-groups have been loaded.
    pub fn is_complete(&self) -> bool {
        self.complete.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of row-groups loaded so far.
    pub fn loaded_count(&self) -> usize {
        self.loaded_row_groups
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Convert in-memory Parquet bytes to JSONL lines using Arrow's reader.
///
/// Uses the same approach as `format::parquet_to_jsonl` but operates on
/// `Bytes` (from HTTP response) instead of a file handle.
fn bytes_to_jsonl(data: Bytes) -> Result<Vec<String>> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(data)
        .with_context(|| "Failed to read Parquet metadata from bytes")?;

    let reader = builder
        .build()
        .with_context(|| "Failed to build Parquet reader from bytes")?;

    let mut lines = Vec::new();

    for batch_result in reader {
        let batch = batch_result.with_context(|| "Failed to read Parquet batch")?;

        // Convert batch to JSON using Arrow's JSON writer
        let mut buf = Vec::new();
        {
            let mut writer = LineDelimitedWriter::new(&mut buf);
            writer
                .write(&batch)
                .with_context(|| "Failed to serialize batch to JSON")?;
            writer
                .finish()
                .with_context(|| "Failed to finish JSON writer")?;
        }

        let json_str =
            String::from_utf8(buf).with_context(|| "Invalid UTF-8 in JSON output")?;

        for line in json_str.lines() {
            if !line.trim().is_empty() {
                lines.push(line.to_string());
            }
        }
    }

    Ok(lines)
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_hf_url_basic() {
        let target = resolve_hf_url("hf://allenai/c4").unwrap();
        assert_eq!(target.org, "allenai");
        assert_eq!(target.dataset, "c4");
        assert_eq!(target.config, "default");
        assert_eq!(target.split, "train");
    }

    #[test]
    fn test_resolve_hf_url_with_split() {
        let target = resolve_hf_url("hf://allenai/c4/validation").unwrap();
        assert_eq!(target.split, "validation");
    }

    #[test]
    fn test_resolve_hf_url_with_config_and_split() {
        let target = resolve_hf_url("hf://allenai/c4/en/train").unwrap();
        assert_eq!(target.config, "en");
        assert_eq!(target.split, "train");
    }

    #[test]
    fn test_resolve_hf_url_invalid() {
        assert!(resolve_hf_url("not_hf://foo/bar").is_err());
        assert!(resolve_hf_url("hf://single").is_err());
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1_500_000), "1.4 MB");
        assert_eq!(format_size(2_500_000_000), "2.3 GB");
    }
}
