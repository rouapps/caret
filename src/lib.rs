//! Caret — High-performance curation engine for LLM training datasets.
//!
//! Provides zero-copy memory-mapped I/O, SIMD-accelerated deduplication,
//! token visualization, and reasoning validation for JSONL, Parquet, and CSV
//! datasets at any scale.
//!
//! ## Connectivity (v0.4)
//!
//! - **MCP Server** (`mcp`) — Expose datasets as Tools/Resources to LLM clients
//!   via the Model Context Protocol (JSON-RPC over HTTP).
//! - **HF Streaming** (`streaming`) — Stream Parquet files directly from the
//!   Hugging Face Hub using HTTP Range requests — no full download needed.

pub mod app;
pub mod data;
pub mod engine;
pub mod fixer;
pub mod format;
pub mod linter;
pub mod mcp;
pub mod streaming;
pub mod tokenizer;
pub mod tui;
pub mod ui;
