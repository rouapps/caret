# Caret

[![Rust 1.75+](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org/)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg)]()

Caret is a terminal tool for inspecting and cleaning large LLM training datasets. It handles JSONL, Parquet, and CSV files using memory-mapped I/O, and includes near-duplicate detection, token visualization, dataset linting, and an [MCP](https://modelcontextprotocol.io/) server.

## Quick Start

```bash
git clone https://github.com/rouapps/caret.git
cd caret && cargo build --release

caret data.jsonl                          # JSONL (memory-mapped)
caret data.parquet                        # Parquet (Arrow-native)
caret data.csv                            # CSV
caret hf://tatsu-lab/alpaca               # Stream from HuggingFace (no download)
caret data.jsonl --mcp-port 3100          # Start MCP server alongside TUI
```

## How It Works

Caret memory-maps the file via `memmap2` and builds a byte-offset index of line boundaries. Line access is O(1) -- the OS page cache handles the rest. Data is never copied into userspace; Caret slices directly into the mapped region.

For remote HuggingFace datasets, Caret fetches only the Parquet footer metadata via HTTP Range requests, then loads row-groups on demand as you scroll.

## Features

- **Memory-mapped I/O** -- files of any size open instantly with near-zero RSS
- **Near-duplicate detection** -- SimHash fingerprinting with hardware `POPCNT`, parallelized via `rayon`
- **HuggingFace Hub streaming** -- browse remote datasets without downloading them
- **MCP server** -- expose dataset tools to Claude Desktop, Cursor, or any MCP client
- **Token X-Ray** -- visualize tokenization boundaries (Tiktoken, HuggingFace, GPT-2)
- **Dataset linter** -- catch unbalanced `<think>` tags, invalid JSON, missing keys
- **Auto-fix** -- repair common formatting issues in JSONL datasets
- **Detail panel** -- split-screen pretty-printed JSON view
- **Pipeline support** -- reads from stdin (`cat data.jsonl | caret -`)

## MCP Server

Caret implements the [Model Context Protocol](https://modelcontextprotocol.io/), exposing these tools:

| Tool | Description |
|------|-------------|
| `search_dataset` | Regex search across the dataset |
| `dataset_info` | Line count, file size, format metadata |
| `get_lines` | Random access to any line range |
| `dedup_scan` | SimHash dedup with statistics |
| `jump_to_line` | Navigate TUI to a specific line |
| `toggle_view` | Cycle view mode (Text / Token X-Ray / Tree) |
| `show_detail` | Show/hide detail panel |

```bash
caret data.jsonl --mcp-port 3100          # TUI + MCP server
caret data.jsonl --mcp-only               # Headless (for CI/pipelines)
```

The TUI control tools (`jump_to_line`, `toggle_view`, `show_detail`) allow AI assistants to interactively navigate the dataset while you watch — ask Claude or Gemini to "jump to line 500 and show the tokens" and the TUI responds instantly.

To use with Claude Desktop, add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "caret": {
      "command": "/path/to/caret",
      "args": ["your_dataset.jsonl", "--mcp-only", "--mcp-port", "3100"]
    }
  }
}
```

## HuggingFace Hub Streaming

```bash
caret hf://allenai/c4                     # Default split
caret hf://tatsu-lab/alpaca/train         # Specific split
caret hf://allenai/c4/en/validation       # Config + split
```

Caret issues a `HEAD` request to get the file size, fetches the 4-byte Parquet footer length, reads the Thrift metadata (a few KB), then loads only the row-groups you scroll to. The first page appears in under a second.

## Deduplication

```bash
caret data.jsonl --dedup                              # Scan and report
caret data.jsonl --dedup --dedup-export clean.jsonl   # Export unique lines
caret data.jsonl --dedup --dedup-strategy exact        # Exact match only
caret data.jsonl --dedup --dedup-threshold 5           # Aggressive fuzzy (0-10, default 3)
```

Press `D` in the TUI to run an interactive dedup scan. Duplicates are highlighted with a `DUP` badge.

The engine works in two phases: parallel fingerprinting (workers read directly from the mmap, hash content into 64-bit SimHash via FNV-1a shingles) followed by index construction (each fingerprint compared via `XOR` + `POPCNT`). Duplicates are tracked in a compact bitmask -- 1 billion lines takes ~125 MB.

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j` / `Down` | Move down |
| `k` / `Up` | Move up |
| `g` / `G` | Top / Bottom |
| `Ctrl+d` / `Ctrl+u` | Page down / up |
| `Tab` | Cycle view: Text / Token X-Ray / Tree |
| `Enter` | Toggle detail panel |
| `D` | Toggle dedup scan |
| `?` | Help |
| `q` | Quit |

## Usage Reference

```bash
# Local files
caret data.jsonl                                      # Auto-detect format
caret data.parquet                                    # Parquet
caret data.csv                                        # CSV
caret data.txt --format jsonl                         # Force format

# HuggingFace
caret hf://org/dataset                                # Default split (train)
caret hf://org/dataset/validation                     # Specific split
caret hf://org/dataset/config/split                   # Config + split

# MCP server
caret data.jsonl --mcp-port 3100                      # TUI + MCP
caret data.jsonl --mcp-only                           # Headless
caret data.jsonl --mcp-only --mcp-port 8080           # Custom port

# Deduplication
caret data.jsonl --dedup                              # Scan and report
caret data.jsonl --dedup --dedup-export clean.jsonl   # Export unique lines
caret data.jsonl --dedup --dedup-strategy exact        # Exact match
caret data.jsonl --dedup --dedup-threshold 5           # Aggressive fuzzy

# Linting
caret data.jsonl --lint
caret data.jsonl --lint --required-keys "messages,prompt"

# Token visualization
caret data.jsonl -t                                   # Tiktoken cl100k_base
caret data.jsonl -t --tiktoken-encoding p50k_base     # Codex encoding
caret data.jsonl -t --tokenizer-type huggingface      # Llama 3.1
caret data.jsonl --tokenizer-path ./my-tokenizer.json # Custom tokenizer

# Auto-fix
caret data.jsonl --fix                                # Creates data_fixed.jsonl
caret data.jsonl --fix -o output.jsonl                # Custom output
caret data.jsonl --fix --fix-in-place                 # Overwrite original

# Pipeline
cat data.jsonl | caret -
```

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                        Caret TUI (Ratatui)                       │
├──────────┬──────────┬──────────────┬──────────┬─────────────────┤
│ Dataset  │Tokenizer │   Linter     │  Dedup   │   MCP Server    │
│  (mmap)  │(Tiktoken)│(Regex+JSON)  │(SimHash) │  (Axum/Tokio)   │
├──────────┼──────────┴──────────────┴──────────┤─────────────────┤
│ HF Stream│     memmap2 · serde_json · rayon    │ reqwest · axum  │
│ (Range)  │                                     │ tower · tracing │
└──────────┴─────────────────────────────────────┴─────────────────┘
```

## Contributing

Contributions welcome. See issues labeled `good first issue`.

```bash
cargo run -- test_data.jsonl              # Development
cargo test                                # Tests
cargo build --release                     # Optimized build
RUST_LOG=caret=debug cargo run -- test_data.jsonl  # Debug logging
```

## Requirements

- Rust 1.75+
- A terminal with 256-color support

## License

MIT -- see [LICENSE](LICENSE).
