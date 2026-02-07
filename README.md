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

Open 50GB+ datasets **instantly**. Visualize token boundaries. Catch data quality issues before they kill your fine-tuning run.

## ✨ Features

### 📁 Instant File Opening
Memory-mapped I/O means your 100GB dataset opens in **0.1 seconds**. No loading bars. No "file too large" errors.

### 🔬 Token X-Ray Mode
Press `Tab` to see exactly how your text tokenizes. Alternating background colors show token boundaries - spot tokenization issues instantly.

### 🧠 Reasoning Linter  
Built for Chain-of-Thought datasets. Automatically detects:
- Unbalanced `<think>`/`</think>` tags
- Invalid JSON/JSONL structure  
- Missing required keys

### 🔧 Auto-Fix Mode (NEW)
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

# Open a dataset
caret your_dataset.jsonl

# With linter
caret your_dataset.jsonl --lint

# With tokenizer (Token X-Ray mode)
caret your_dataset.jsonl --tokenizer path/to/tokenizer.json
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
# Basic usage
caret data.jsonl

# Enable linting
caret data.jsonl --lint

# Lint with required keys check
caret data.jsonl --lint --required-keys "messages,prompt"

# Token visualization (requires tokenizer.json)
caret data.jsonl --tokenizer ./llama3-tokenizer.json

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
| See token boundaries | ❌ | ❌ | ✅ |
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

- **Memory Mapping**: Zero-copy file access via `memmap2`
- **Line Indexing**: O(1) access to any line in the file
- **Tokenization**: Native Rust bindings to HuggingFace tokenizers
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
