# I built a Rust data engine that lets Claude search your 50GB datasets. Here's the systems engineering.

**TL;DR**: Caret is a Rust-native dataset engine for LLM training data. It opens 50GB+ files instantly via mmap, deduplicates with SIMD POPCNT, and now — in v0.4 — it can stream Parquet from HuggingFace without downloading, and expose your data to LLMs as an MCP server. It's open-source (MIT). Here's how every piece works under the hood.

---

## The Problem (Still)

If you've fine-tuned a model, you know the pain. You have a 20GB JSONL file. You need to:

1. **Inspect it** — but VS Code crashes, `jq` is slow, and Python pandas loads the entire thing into RAM.
2. **Check for broken data** — unbalanced `<think>` tags, malformed JSON, missing keys. One bad line can tank a training run.
3. **Deduplicate it** — duplicate training examples cause overfitting. But running MinHash in Python takes hours on large datasets.
4. **Share it with your AI tools** — Claude and Cursor can't see your local files. You end up copy-pasting samples.
5. **Browse remote datasets** — HuggingFace has thousands of public datasets, but you have to download them entirely before inspecting anything.

Caret solves all five. Let me show you how.

## The Zero-Copy Foundation

The core insight: **your data should never be copied**.

When you do `pd.read_json("data.jsonl", lines=True)` in Python, here's what happens:
1. The OS reads the file from disk into kernel page cache.
2. Python's `read()` **copies** it from kernel space to a Python `bytes` object in user space.
3. pandas **copies** it again into a DataFrame with Python object overhead per cell.

That's 2-3x the file size in memory, plus minutes of CPU time for the copies and allocations.

Caret does this instead:

```rust
let mmap = unsafe { Mmap::map(&file)? };
```

One line. The file is now accessible as a `&[u8]` slice backed by the OS page cache. No copies. No allocations. No waiting. A 50GB file "opens" in **0.003 seconds** because `mmap()` is O(1) — it just sets up virtual memory mappings.

Then we build a line index:

```rust
let mut line_offsets = vec![0];
for (i, &byte) in mmap.iter().enumerate() {
    if byte == b'\n' && i + 1 < mmap.len() {
        line_offsets.push(i + 1);
    }
}
```

This is a single sequential scan — cache-friendly, branch-predictor-friendly. After this, accessing any line is O(1):

```rust
fn get_line(&self, index: usize) -> &str {
    let start = self.line_offsets[index];
    let end = self.line_offsets[index + 1] - 1;
    std::str::from_utf8(&self.mmap[start..end]).unwrap()
}
```

The returned `&str` is a **view into the mmap**. No allocation. No copy. The OS page cache handles all I/O transparently — pages that aren't in RAM get faulted in on demand.

## The Dedup Engine: SimHash + POPCNT

Here's the architecture:

### Phase 1: Parallel Fingerprinting

Each line in the dataset gets a 64-bit SimHash fingerprint. The key property of SimHash: **similar documents produce similar fingerprints**. The number of differing bits (Hamming distance) between two fingerprints correlates with the semantic distance between the documents.

The algorithm:

1. Slide a 4-byte window (shingle) across the input
2. Hash each shingle with FNV-1a (zero-alloc, branch-free inner loop)
3. For each of the 64 bit positions in the hash, add +1 or -1 to an accumulator
4. After all shingles: each bit position becomes 1 if its accumulator is positive, 0 otherwise

```rust
pub fn fingerprint(&self, data: &[u8]) -> Fingerprint {
    let mut acc = [0i32; 64];
    for window in data.windows(self.shingle_size) {
        let hash = self.fnv1a(window);
        for i in 0..64 {
            if hash & (1u64 << i) != 0 {
                acc[i] += 1;
            } else {
                acc[i] -= 1;
            }
        }
    }
    // Collapse to 64-bit fingerprint
    let mut fp: u64 = 0;
    for (i, &val) in acc.iter().enumerate() {
        if val > 0 { fp |= 1u64 << i; }
    }
    Fingerprint(fp)
}
```

The critical detail: **this runs in parallel via rayon**, and each worker thread reads directly from the memory-mapped file. Zero copies. Zero allocations (except the fingerprint Vec itself). The compiler auto-vectorizes the accumulator loop at `-C opt-level=3`.

But before fingerprinting, we need to extract the actual text content from JSON. Instead of deserializing into `serde_json::Value` (which allocates a tree of heap objects), we use a custom byte-level scanner:

```rust
fn extract_content_bytes(data: &[u8]) -> Vec<u8> {
    // Scan for string values (after ':') without JSON deserialization
    // ~5x faster than serde for fingerprinting purposes
}
```

This scans the raw JSON bytes, extracts only string values (skipping keys and structural characters), and returns a flat byte buffer. No `Value` tree. No `HashMap`. No `String` allocations for keys.

### Phase 2: Hamming Distance Index

Once we have fingerprints, we need to find near-duplicates. Two lines are near-duplicates if their fingerprints differ by fewer than `threshold` bits.

The Hamming distance between two 64-bit integers is:

```rust
fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
```

`count_ones()` compiles to a single `POPCNT` instruction on x86_64. That's **one XOR + one POPCNT** per comparison — sub-nanosecond. We can check millions of fingerprint pairs per second.

### The Result: A BitMask

Duplicate lines are stored in a compact bitmask:

```rust
pub struct BitMask {
    words: Vec<u64>,  // 64 lines per word
    len: usize,
}
```

One billion lines = 125 MB of bitmask. Set a bit: `words[i >> 6] |= 1u64 << (i & 63)`. Test a bit: same operation with `&` instead of `|=`. O(1), cache-friendly, and the bit-shift operations are essentially free on modern CPUs.

## NEW: The MCP Server — Giving LLMs Eyes on Your Data

This is the v0.4 feature I'm most excited about. **Caret is now an MCP (Model Context Protocol) server.**

MCP is the protocol that lets Claude Desktop, Cursor, and other AI tools call external tools. Caret implements the JSON-RPC 2.0 surface over HTTP using Axum:

```
LLM Client ──JSON-RPC──▶ Axum Router ──zero-copy──▶ Dataset (mmap)
```

The key architectural decision: the MCP server runs on a **background Tokio task** while the TUI runs on the main thread. They share the dataset via `Arc<RwLock<Dataset>>`, so the TUI never freezes while Claude is searching your data.

```rust
// Background MCP server alongside the TUI
rt.spawn(mcp::start_mcp_server(dataset_arc, dataset_path, port));
```

### What the LLM Can Do

Four tools are exposed:

1. **`search_dataset(query)`** — Regex search. The `regex` crate uses SIMD acceleration on x86_64. The search runs on `tokio::task::spawn_blocking` so it doesn't block the async runtime.

2. **`get_lines(start, count)`** — O(1) random access via the byte-offset table. An LLM can jump to line 50,000,000 in a 50GB file in nanoseconds.

3. **`dataset_info()`** — Line count, file size, format. Context for the LLM to reason about scale.

4. **`dedup_scan(strategy, threshold)`** — The full SimHash engine. An LLM can say "check for duplicates" and get back statistics + sample pairs.

### Why This Matters

Before MCP, if you asked Claude "are there duplicates in my training data?", it couldn't help. Now:

```
You: "Search my 10GB dataset for examples about quantum computing"
Claude: [calls search_dataset("quantum")] → 847 matches in 0.3s

You: "How many duplicates are there?"
Claude: [calls dedup_scan("simhash", 3)] → 12.4% duplicates, 8.2s scan

You: "Show me lines 500-510"
Claude: [calls get_lines(499, 11)] → instant O(1) access
```

The LLM gets structured, fast access to your data. No copy-pasting. No sampling. The full dataset, the full engine.

## NEW: HuggingFace Streaming — Range Requests, Not Downloads

The second v0.4 feature: you can now browse remote Parquet datasets without downloading them.

```bash
caret hf://allenai/c4
```

### The Protocol

Parquet files have their metadata at the **end** of the file. This is a gift for streaming:

```
[row-group 0][row-group 1]...[row-group N][footer][4-byte len][PAR1]
```

Caret uses HTTP Range requests to read just the pieces it needs:

1. **HEAD** → get `Content-Length` (total file size). Zero bytes transferred.
2. **Range: bytes=(size-8)-(size-1)** → read the last 8 bytes: 4-byte footer length + "PAR1" magic. 8 bytes transferred.
3. **Range: bytes=(size-footer_len-8)-(size-9)** → read the full Thrift-encoded footer. A few KB typically.
4. Parse the footer with `ParquetMetaDataReader::decode_metadata()` to discover row-group offsets, column schemas, and row counts.
5. **Range: bytes=offset-(offset+size)** → fetch only the row-groups the user scrolls to.

The first row-group displays in under a second. For multi-group files, the remaining groups load in the background via `tokio::spawn`:

```rust
tokio::spawn(async move {
    for i in 1..total_rgs {
        let lines = reader.fetch_row_group(&meta, i).await?;
        lines_bg.write().await.extend(lines);
        loaded_bg.store(i + 1, Ordering::Relaxed);
    }
    complete_bg.store(true, Ordering::Relaxed);
});
```

The TUI is already interactive while background groups are still downloading. No blocking. No freezing.

### Why Not Just Download?

The C4 dataset is 300GB+ of Parquet files. At 100 Mbps, that's 7+ hours to download. With Range requests, you're browsing in under 2 seconds. You only fetch what you look at.

## The Async Architecture

v0.4 required careful integration of sync and async code. The TUI event loop (crossterm polling) is synchronous. The MCP server and HF streaming are async (tokio). Here's how they coexist:

```rust
// Build the tokio runtime once
let rt = tokio::runtime::Runtime::new()?;

// HF streaming: block_on (we need the data before showing the TUI)
let dataset = rt.block_on(streaming::open_hf_stream(&args.file))?;

// MCP server: spawn (runs in background, TUI stays responsive)
rt.spawn(mcp::start_mcp_server(dataset_arc, dataset_path, port));

// TUI event loop: synchronous crossterm polling at 60fps
loop {
    tui.terminal().draw(|frame| ui::render(frame, &mut app))?;
    if event::poll(Duration::from_millis(16))? { /* handle keys */ }
}
```

The Tokio runtime lives for the program's lifetime. CPU-intensive operations (regex search, SimHash fingerprinting) are offloaded to `spawn_blocking` so they don't starve the async I/O tasks.

## The Numbers

On a MacBook Pro (M-series), rough benchmarks:

| Operation | Caret | Python Equivalent |
|---|---|---|
| Open 10GB JSONL | 0.003s | 45s (pandas) |
| Scroll to line 50,000,000 | instant (O(1)) | N/A (pandas loads all) |
| Dedup 1M lines (SimHash) | ~2s | ~90s (datasketch MinHash) |
| Dedup 1M lines (exact) | <1s | ~30s (set-based) |
| Memory for 50GB file | ~0 (page cache) | 50-100GB |
| MCP search (regex, 10M lines) | ~300ms | N/A |
| HF stream time-to-first-row | ~1s | N/A (full download) |

The memory number is the most important one. With mmap, the OS manages memory pressure automatically. If your system needs RAM for something else, it just evicts pages from the page cache. No OOM. No swap thrashing. Your 16GB laptop can work with a 100GB dataset.

## Try It

```bash
git clone https://github.com/rayanouaddi/caret
cd caret
cargo build --release

# Local files
./target/release/caret your_dataset.jsonl --dedup

# Stream from HuggingFace
./target/release/caret hf://tatsu-lab/alpaca

# MCP server for Claude/Cursor
./target/release/caret your_dataset.jsonl --mcp-port 3100
```

Press `D` in the TUI to run an interactive dedup scan. Duplicate lines light up in amber. Press `Tab` for token visualization. Press `?` for help.

## What's Next

- **Overlay mutations**: Copy-on-write editing layer over the mmap (edit a 100GB file with kilobytes of overhead)
- **LSH indexing**: Locality-sensitive hashing for sub-linear dedup on billion-line datasets
- **MCP stdio transport**: Direct pipe integration for Claude Desktop (no HTTP needed)
- **Incremental TUI streaming**: Display row-groups as they arrive, with a loading indicator
- **Multi-file merge/split**: Combine and partition datasets with dedup across files

MIT licensed. PRs welcome. Built for the community that spends more time cleaning data than training models.

---

**Links**: [GitHub](https://github.com/rayanouaddi/caret) | MIT License
