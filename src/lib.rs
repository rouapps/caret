//! Caret — High-performance curation engine for LLM training datasets.
//!
//! Provides zero-copy memory-mapped I/O, SIMD-accelerated deduplication,
//! token visualization, and reasoning validation for JSONL, Parquet, and CSV
//! datasets at any scale.

pub mod app;
pub mod data;
pub mod engine;
pub mod fixer;
pub mod format;
pub mod linter;
pub mod tokenizer;
pub mod tui;
pub mod ui;
