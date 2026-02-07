# 🚀 LazyAlign

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

## 🚀 Quick Start

```bash
# Install from source
cargo install --path .

# Open a dataset
lazyalign your_dataset.jsonl

# With linter
lazyalign your_dataset.jsonl --lint

# With tokenizer (Token X-Ray mode)
lazyalign your_dataset.jsonl --tokenizer path/to/tokenizer.json
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
| `Tab` | Toggle Token X-Ray mode |
| `?` | Show help |
| `q` | Quit |

## 📦 Installation

### From Source (Recommended)

```bash
git clone https://github.com/yourusername/lazyalign
cd lazyalign
cargo build --release
./target/release/lazyalign --help
```

### Requirements
- Rust 1.75+
- A terminal with 256-color support

## 🔧 Usage

```bash
# Basic usage
lazyalign data.jsonl

# Enable linting
lazyalign data.jsonl --lint

# Lint with required keys check
lazyalign data.jsonl --lint --required-keys "messages,prompt"

# Token visualization (requires tokenizer.json)
lazyalign data.jsonl --tokenizer ./llama3-tokenizer.json
```

## 🎯 Why LazyAlign?

Fine-tuning LLMs is brutally unforgiving. A single malformed JSON line or unbalanced reasoning tag can tank your training run and waste thousands of dollars in compute.

**LazyAlign catches these issues before they cost you:**

| Problem | VS Code | jq | LazyAlign |
|---------|---------|----|---------| 
| Open 10GB file | ❌ Crashes | ✅ Slow | ✅ Instant |
| See token boundaries | ❌ | ❌ | ✅ |
| Find broken `<think>` tags | Manual | ❌ | ✅ Auto |
| Smooth scrolling | ❌ | ❌ | ✅ 60 FPS |

## 📐 Architecture

```
┌──────────────────────────────────────────────────────┐
│                    LazyAlign TUI                      │
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
