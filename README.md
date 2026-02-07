# 🚀 Caret

<p align="center">
  <b>Blazingly fast TUI for inspecting and curating LLM training datasets</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.75+-orange.svg" alt="Rust 1.75+">
  <img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License">
  <img src="https://img.shields.io/badge/platform-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg" alt="Platform">
</p>

---

Open 50GB+ datasets **instantly**. Supports **JSONL, Parquet, and CSV**. Visualize token boundaries with **Tiktoken cl100k_base**. Catch data quality issues before they kill your fine-tuning run.

## ✨ Features

### 📁 Multi-Format Support
- **JSONL**: Memory-mapped I/O - 100GB files open in **0.1 seconds**
- **Parquet**: Native Apache Arrow support for columnar datasets
- **CSV**: Auto-converts CSV rows to JSON objects

### 🔬 Token X-Ray Mode
Press `Tab` to see exactly how your text tokenizes. Uses **Tiktoken cl100k_base** (GPT-4 tokenizer) by default. Alternating background colors show token boundaries.

### 🧠 Reasoning Linter  
Built for Chain-of-Thought datasets. Automatically detects:
- Unbalanced `<think>`/`</think>` tags
- Invalid JSON/JSONL structure  
- Missing required keys

### 🔧 Auto-Fix Mode (EXPERIMENTAL)
Automatically repair common dataset issues:
```bash
caret data.jsonl --fix              # Creates data_fixed.jsonl
caret data.jsonl --fix -o clean.jsonl  # Custom output path
```

### 📝 Detail Panel
Press `Enter` to open a split-screen view with pretty-printed JSON. Navigate deep nested structures without squinting at minified data.

### 🔗 Pipeline Support
Fully compatible with Unix pipelines:
```bash
cat huge_dataset.jsonl | caret -
curl https://example.com/data.jsonl | caret -
```

## 🚀 Quick Start

```bash
# Install from source
cargo install --path .

# Open any dataset format
caret data.jsonl           # JSONL (default)
caret data.parquet          # Parquet (auto-detected)
caret data.csv              # CSV (auto-detected)

# With linter
caret data.jsonl --lint

# With tokenizer (Token X-Ray mode using Tiktoken cl100k_base)
caret data.jsonl -t

# Use HuggingFace tokenizer instead
caret data.jsonl -t --tokenizer-type huggingface
```

## ⌨️ Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` | Go to top |
| `G` | Go to bottom |
| `Ctrl+d` | Page down |
| `Ctrl+u` | Page up |
| `Tab` | Cycle view: TEXT → TOKEN X-RAY → TREE |
| `Enter` | Toggle detail panel (pretty JSON) |
| `?` | Show help |
| `q` | Quit |

## 📦 Installation

### From Source (Recommended)

```bash
git clone https://github.com/yourusername/caret
cd caret
cargo build --release
./target/release/caret --help
```

### Requirements
- Rust 1.75+
- A terminal with 256-color support

## 🔧 Usage

```bash
# Basic usage (auto-detects format from extension)
caret data.jsonl
caret data.parquet
caret data.csv

# Force specific format
caret data.txt --format jsonl

# Enable linting
caret data.jsonl --lint

# Lint with required keys check
caret data.jsonl --lint --required-keys "messages,prompt"

# Token visualization (Tiktoken cl100k_base - default)
caret data.jsonl -t

# Use different Tiktoken encoding
caret data.jsonl -t --tiktoken-encoding p50k_base

# Use HuggingFace tokenizer (Llama 3.1)
caret data.jsonl -t --tokenizer-type huggingface

# Use legacy GPT-2 tokenizer
caret data.jsonl -t --tokenizer-type gpt2

# Custom tokenizer file
caret data.jsonl --tokenizer-path ./my-tokenizer.json

# Pipeline mode (read from stdin)
cat data.jsonl | caret -

# Auto-fix mode (headless, creates new file)
caret data.jsonl --fix                 # → data_fixed.jsonl
caret data.jsonl --fix -o output.jsonl # Custom output
caret data.jsonl --fix --fix-in-place  # Overwrite original (careful!)
caret data.jsonl --fix --skip-invalid  # Skip unfixable lines
```

## 🎯 Why Caret?

Fine-tuning LLMs is brutally unforgiving. A single malformed JSON line or unbalanced reasoning tag can tank your training run and waste thousands of dollars in compute.

**Caret catches these issues before they cost you:**

| Problem | VS Code | jq | Caret |
|---------|---------|----|---------| 
| Open 10GB file | ❌ Crashes | ✅ Slow | ✅ Instant |
| Load Parquet | ❌ | ❌ | ✅ Native |
| See token boundaries | ❌ | ❌ | ✅ Tiktoken |
| Find broken `<think>` tags | Manual | ❌ | ✅ Auto |
| Smooth scrolling | ❌ | ❌ | ✅ 60 FPS |

## 📐 Architecture

```
┌──────────────────────────────────────────────────────┐
│                      Caret TUI                       │
├──────────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
│  │ Dataset  │  │Tokenizer │  │    Linter        │   │
│  │  (mmap)  │  │ (HF Rust)│  │ (Regex + JSON)   │   │
│  └──────────┘  └──────────┘  └──────────────────┘   │
├──────────────────────────────────────────────────────┤
│              Ratatui + Crossterm                     │
└──────────────────────────────────────────────────────┘
```

- **Memory Mapping**: Zero-copy file access via `memmap2` (JSONL)
- **Parquet/CSV**: Arrow-native conversion to JSONL in memory
- **Line Indexing**: O(1) access to any line in the file
- **Tokenization**: Tiktoken (cl100k_base) + HuggingFace tokenizers
- **Rendering**: Immediate-mode TUI with 60 FPS scrolling

## 🤝 Contributing

Contributions welcome! Check out the issues labeled `good first issue`.

```bash
# Run in development mode
cargo run -- test_data.jsonl

# Run tests
cargo test

# Build optimized release
cargo build --release
```

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built with 🦀 Rust and ❤️ for the LLM community
</p>
# caret
