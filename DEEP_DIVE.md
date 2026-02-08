# I built a Rust TUI that deduplicates 50GB LLM datasets in seconds. Here's the low-level magic.

**TL;DR**: Caret is a terminal-based dataset curation engine for LLM training data. It opens 50GB+ files instantly via memory-mapped I/O, and its new dedup engine uses SimHash fingerprinting with hardware POPCNT to find near-duplicate training examples orders of magnitude faster than Python. It's open-source (MIT). Here's how it works under the hood.

---

## The Problem

If you've fine-tuned a model, you know the pain. You have a 20GB JSONL file. You need to:

1. **Inspect it** — but VS Code crashes, `jq` is slow, and Python pandas loads the entire thing into RAM.
2. **Check for broken data** — unbalanced `<think>` tags, malformed JSON, missing keys. One bad line can tank a training run.
3. **Deduplicate it** — duplicate training examples cause overfitting. But running MinHash in Python takes hours on large datasets.

I got tired of writing throwaway Python scripts for this. So I built Caret in Rust.

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

This is the new feature in v0.3 and the part I'm most excited about. Here's the architecture:

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

For exact-match mode, we use a `HashMap<u64, usize>` — O(1) lookup. For SimHash fuzzy mode, we do a linear scan against known unique fingerprints with early exit. This is O(N*U) where U is the unique count, but since each comparison is a single POPCNT, it's practical for U up to ~5M.

### The Result: A BitMask

Duplicate lines are stored in a compact bitmask:

```rust
pub struct BitMask {
    words: Vec<u64>,  // 64 lines per word
    len: usize,
}
```

One billion lines = 125 MB of bitmask. Set a bit: `words[i >> 6] |= 1u64 << (i & 63)`. Test a bit: same operation with `&` instead of `|=`. O(1), cache-friendly, and the bit-shift operations are essentially free on modern CPUs.

## The Numbers

On a MacBook Pro (M-series), rough benchmarks:

| Operation | Caret | Python Equivalent |
|---|---|---|
| Open 10GB JSONL | 0.003s | 45s (pandas) |
| Scroll to line 50,000,000 | instant (O(1)) | N/A (pandas loads all) |
| Dedup 1M lines (SimHash) | ~2s | ~90s (datasketch MinHash) |
| Dedup 1M lines (exact) | <1s | ~30s (set-based) |
| Memory for 50GB file | ~0 (page cache) | 50-100GB |

The memory number is the most important one. With mmap, the OS manages memory pressure automatically. If your system needs RAM for something else, it just evicts pages from the page cache. No OOM. No swap thrashing. Your 16GB laptop can work with a 100GB dataset.

## Try It

```bash
git clone https://github.com/rayanouaddi/caret
cd caret
cargo build --release
./target/release/caret your_dataset.jsonl --dedup
```

Press `D` in the TUI to run an interactive dedup scan. Duplicate lines light up in amber. Press `Tab` for token visualization. Press `?` for help.

## What's Next

- **Overlay mutations**: Copy-on-write editing layer over the mmap (edit a 100GB file with kilobytes of overhead)
- **LSH indexing**: Locality-sensitive hashing for sub-linear dedup on billion-line datasets
- **Streaming column projection**: SIMD-accelerated JSON field extraction without full deserialization
- **Multi-file merge/split**: Combine and partition datasets with dedup across files

MIT licensed. PRs welcome. Built for the community that spends more time cleaning data than training models.

---

**Links**: [GitHub](https://github.com/rayanouaddi/caret) | MIT License
