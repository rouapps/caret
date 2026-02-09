//! Caret - A blazingly fast TUI for inspecting LLM datasets
//!
//! Provides instant file opening for massive JSONL, Parquet, and CSV files,
//! token visualization, reasoning data validation, SIMD-accelerated
//! near-duplicate detection, MCP server connectivity, and HuggingFace
//! Hub streaming.

use anyhow::{Context, Result};
use argh::FromArgs;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use caret::app::{App, ViewMode};
use caret::commands::{command_channel, TuiCommand, TuiCommandReceiver, ViewModeCmd};
use caret::data::Dataset;
use caret::engine::{DedupEngine, DedupStrategy};
use caret::fixer::{FixResult, Fixer, FixSummary};
use caret::format::InputFormat;
use caret::linter::Linter;
use caret::mcp;
use caret::streaming;
use caret::tokenizer::{TiktokenEncoding, TokenizerType, TokenizerWrapper};
use caret::tui::Tui;
use caret::ui;

/// Caret - Blazingly fast TUI for LLM dataset curation
#[derive(FromArgs)]
struct Args {
    /// path to the dataset file (JSONL, Parquet, CSV, or hf://org/dataset)
    #[argh(positional)]
    file: String,

    /// input format: auto, jsonl, parquet, csv (default: auto-detect)
    #[argh(option, default = "String::from(\"auto\")")]
    format: String,

    /// enable Token X-Ray mode
    #[argh(switch, short = 't')]
    tokenizer: bool,

    /// tokenizer type: tiktoken, huggingface, gpt2 (default: tiktoken)
    #[argh(option, default = "String::from(\"tiktoken\")")]
    tokenizer_type: String,

    /// tiktoken encoding: cl100k_base, p50k_base, r50k_base (default: cl100k_base)
    #[argh(option, default = "String::from(\"cl100k_base\")")]
    tiktoken_encoding: String,

    /// path to custom tokenizer.json (overrides --tokenizer-type)
    #[argh(option)]
    tokenizer_path: Option<String>,

    /// run the linter on load
    #[argh(switch, short = 'l')]
    lint: bool,

    /// required keys to check for (comma-separated)
    #[argh(option, short = 'k')]
    required_keys: Option<String>,

    /// fix mode: auto-repair issues and write to output file
    #[argh(switch, short = 'f')]
    fix: bool,

    /// output path for fixed file (default: {input}_fixed.jsonl)
    #[argh(option, short = 'o')]
    fix_output: Option<String>,

    /// skip lines that cannot be fixed (instead of failing)
    #[argh(switch)]
    skip_invalid: bool,

    /// fix in place (overwrite original file) - USE WITH CAUTION
    #[argh(switch)]
    fix_in_place: bool,

    /// run dedup scan (near-duplicate detection)
    #[argh(switch)]
    dedup: bool,

    /// dedup strategy: exact, simhash (default: simhash)
    #[argh(option, default = "String::from(\"simhash\")")]
    dedup_strategy: String,

    /// simhash hamming distance threshold (0=exact hash, 3=fuzzy, 5=aggressive; default: 3)
    #[argh(option, default = "3")]
    dedup_threshold: u32,

    /// export deduplicated dataset to this path
    #[argh(option)]
    dedup_export: Option<String>,

    /// start MCP server on this port (exposes dataset as tools/resources to LLMs)
    #[argh(option, default = "0")]
    mcp_port: u16,

    /// run MCP server only (headless, no TUI)
    #[argh(switch)]
    mcp_only: bool,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    // Initialize tracing for async subsystems (MCP + streaming)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("caret=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Check if this is a HF streaming URL
    let is_hf_stream = args.file.starts_with("hf://");

    // Build the tokio runtime — used for MCP server and HF streaming.
    // We create it once and reuse it throughout the program lifetime.
    let rt = tokio::runtime::Runtime::new()
        .with_context(|| "Failed to create async runtime")?;

    // Load the dataset — either local file, stdin, or HF stream
    let dataset = if is_hf_stream {
        eprintln!("Streaming from {}...", args.file);
        let (dataset, meta) = rt.block_on(streaming::open_hf_stream(&args.file))
            .with_context(|| format!("Failed to stream from {}", args.file))?;
        eprintln!(
            "Streamed {} lines ({}) — {} row-groups, {} columns",
            dataset.line_count(),
            dataset.size_human(),
            meta.num_row_groups,
            meta.columns.len(),
        );
        dataset
    } else if args.file == "-" {
        eprintln!("Reading from stdin...");
        let dataset = Dataset::from_stdin().with_context(|| "Failed to read from stdin")?;
        eprintln!(
            "Loaded {} lines ({}) from stdin",
            dataset.line_count(),
            dataset.size_human()
        );
        dataset
    } else {
        // Determine input format
        let input_format = if args.format == "auto" {
            InputFormat::detect(&args.file)
        } else {
            InputFormat::parse(&args.format).unwrap_or_else(|| {
                eprintln!("Warning: Unknown format '{}', using auto-detect", args.format);
                InputFormat::detect(&args.file)
            })
        };

        eprintln!("Opening {} as {}...", args.file, match input_format {
            InputFormat::Jsonl => "JSONL",
            InputFormat::Parquet => "Parquet",
            InputFormat::Csv => "CSV",
        });
        let dataset = Dataset::open_with_format(&args.file, input_format)
            .with_context(|| format!("Failed to open dataset: {}", args.file))?;
        eprintln!(
            "Loaded {} lines ({}) as {}",
            dataset.line_count(),
            dataset.size_human(),
            dataset.format_name()
        );
        dataset
    };

    // Create the app
    let mut app = App::new(dataset);

    // Load tokenizer if requested
    if let Some(ref tokenizer_path) = args.tokenizer_path {
        // Custom tokenizer path takes priority
        eprintln!("Loading tokenizer from {}...", tokenizer_path);
        match TokenizerWrapper::from_file(tokenizer_path) {
            Ok(tokenizer) => {
                eprintln!("Tokenizer loaded: {}", tokenizer.name);
                app = app.with_tokenizer(tokenizer);
            }
            Err(e) => {
                eprintln!("Failed to load tokenizer: {}", e);
            }
        }
    } else if args.tokenizer {
        // Determine tokenizer type from CLI
        let tokenizer_type = TokenizerType::parse(&args.tokenizer_type)
            .unwrap_or(TokenizerType::Tiktoken);

        match tokenizer_type {
            TokenizerType::Tiktoken => {
                let encoding = TiktokenEncoding::parse(&args.tiktoken_encoding)
                    .unwrap_or(TiktokenEncoding::Cl100kBase);
                eprintln!("Loading Tiktoken tokenizer ({:?})...", encoding);
                match TokenizerWrapper::from_tiktoken(encoding) {
                    Ok(tokenizer) => {
                        eprintln!("Tokenizer loaded: {}", tokenizer.name);
                        app = app.with_tokenizer(tokenizer);
                    }
                    Err(e) => {
                        eprintln!("Failed to load Tiktoken tokenizer: {}", e);
                    }
                }
            }
            TokenizerType::HuggingFace => {
                eprintln!("Loading HuggingFace tokenizer (Llama 3.1)...");
                match TokenizerWrapper::from_pretrained("meta-llama/Llama-3.1-8B") {
                    Ok(tokenizer) => {
                        eprintln!("Tokenizer loaded: {}", tokenizer.name);
                        app = app.with_tokenizer(tokenizer);
                    }
                    Err(e) => {
                        eprintln!("Failed to load tokenizer: {}", e);
                        eprintln!("  Tip: You may need to accept Llama 3.1's license");
                    }
                }
            }
            TokenizerType::Gpt2 => {
                eprintln!("Loading GPT-2 tokenizer (legacy)...");
                match TokenizerWrapper::from_pretrained("gpt2") {
                    Ok(tokenizer) => {
                        eprintln!("Tokenizer loaded: {}", tokenizer.name);
                        app = app.with_tokenizer(tokenizer);
                    }
                    Err(e) => {
                        eprintln!("Failed to load GPT-2 tokenizer: {}", e);
                    }
                }
            }
        }
    }

    // Run linter if requested
    if args.lint {
        eprintln!("Running linter...");
        let mut linter = Linter::new();
        if let Some(ref keys) = args.required_keys {
            let keys: Vec<String> = keys.split(',').map(|s| s.trim().to_string()).collect();
            linter = linter.with_required_keys(keys);
        }
        let results = linter.lint_dataset(&app.dataset);
        eprintln!("Found {} issues", results.len());
        app = app.with_lint_results(results);
    }

    // Run fix mode if requested (headless, no TUI)
    if args.fix {
        return run_fix_mode(&args, &app.dataset);
    }

    // Run dedup mode if requested (headless, no TUI)
    if args.dedup {
        return run_dedup_mode(&args, &app.dataset);
    }

    // ── MCP Server ──────────────────────────────────────────────────────
    // If --mcp-port is set, start the MCP server as a background task.
    // The server runs on a separate Tokio task and never blocks the TUI.
    let mcp_port = if args.mcp_port > 0 {
        args.mcp_port
    } else if args.mcp_only {
        3100 // Default MCP port
    } else {
        0
    };

    // Initialize TUI (needed for both MCP and non-MCP paths)
    let tui = Tui::new()?;

    if mcp_port > 0 || args.mcp_only {
        // Snapshot the dataset into an Arc for the async MCP server.
        // One-time copy cost — the server then holds a read-only reference.
        let dataset_arc = {
            let mut buf = Vec::new();
            for i in 0..app.dataset.line_count() {
                if let Some(line) = app.dataset.get_line(i) {
                    buf.extend_from_slice(line.as_bytes());
                    buf.push(b'\n');
                }
            }
            let mut offsets = vec![0usize];
            for (i, &b) in buf.iter().enumerate() {
                if b == b'\n' && i + 1 < buf.len() {
                    offsets.push(i + 1);
                }
            }
            let size = buf.len() as u64;
            Arc::new(Dataset::from_raw_parts(
                buf,
                offsets,
                app.dataset.path.clone(),
                size,
                app.dataset.format,
            ))
        };

        let dataset_path = app.dataset.path.clone();
        let port = if mcp_port > 0 { mcp_port } else { 3100 };

        // Create command channel for MCP → TUI communication
        let (tui_tx, tui_rx) = command_channel();

        if args.mcp_only {
            // Headless MCP-only mode — block on the server (no TUI commands)
            eprintln!("Starting MCP server (headless) on port {}...", port);
            rt.block_on(mcp::start_mcp_server(dataset_arc, dataset_path, port, None))?;
            return Ok(());
        } else {
            // Background MCP server alongside the TUI
            eprintln!("Starting MCP server on port {}...", port);
            rt.spawn(mcp::start_mcp_server(dataset_arc, dataset_path, port, Some(tui_tx)));
            
            // Store receiver for the TUI loop
            return run_tui_with_mcp(tui, app, tui_rx);
        }
    }

    // No MCP server — run TUI without command channel
    run_tui_loop(tui, app, None)
}

/// Run TUI with MCP command channel
fn run_tui_with_mcp(mut tui: Tui, mut app: App, tui_rx: TuiCommandReceiver) -> Result<()> {
    run_tui_loop(tui, app, Some(tui_rx))
}

/// Main TUI event loop with optional MCP command channel
fn run_tui_loop(mut tui: Tui, mut app: App, mut tui_rx: Option<TuiCommandReceiver>) -> Result<()> {
    loop {
        // Poll MCP command channel (non-blocking)
        if let Some(ref mut rx) = tui_rx {
            while let Ok(cmd) = rx.try_recv() {
                apply_tui_command(&mut app, cmd);
            }
        }

        // Render
        tui.terminal().draw(|frame| ui::render(frame, &mut app))?;

        // Handle keyboard events
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match (key.code, key.modifiers) {
                    // Quit
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
                        app.should_quit = true;
                    }

                    // Navigation
                    (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                        app.scroll_down(1);
                    }
                    (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                        app.scroll_up(1);
                    }
                    (KeyCode::Char('g'), _) => {
                        app.goto_top();
                    }
                    (KeyCode::Char('G'), _) => {
                        app.goto_bottom();
                    }
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        app.scroll_down(app.viewport_height / 2);
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        app.scroll_up(app.viewport_height / 2);
                    }
                    (KeyCode::PageDown, _) => {
                        app.scroll_down(app.viewport_height);
                    }
                    (KeyCode::PageUp, _) => {
                        app.scroll_up(app.viewport_height);
                    }

                    // Toggle Token X-Ray mode
                    (KeyCode::Tab, _) => {
                        app.toggle_view_mode();
                    }

                    // Toggle detail panel
                    (KeyCode::Enter, _) => {
                        app.toggle_detail();
                    }

                    // Toggle dedup scan (Shift+D)
                    (KeyCode::Char('D'), _) => {
                        app.toggle_dedup();
                    }

                    // Toggle help
                    (KeyCode::Char('?'), _) => {
                        app.show_help = !app.show_help;
                    }

                    _ => {}
                }
            }

            if let Event::Resize(_, _) = event::read().unwrap_or(Event::FocusGained) {
                // Resize handled automatically by ratatui
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Cleanup
    tui.restore()?;
    eprintln!("Goodbye!");

    Ok(())
}

/// Apply a TUI command from MCP to the app state
fn apply_tui_command(app: &mut App, cmd: TuiCommand) {
    match cmd {
        TuiCommand::JumpToLine(line) => {
            // Jump to line and center it
            app.selected_line = line.min(app.dataset.line_count().saturating_sub(1));
            app.scroll = app.selected_line.saturating_sub(app.viewport_height / 2);
        }
        TuiCommand::ToggleView => {
            app.toggle_view_mode();
        }
        TuiCommand::SetViewMode(mode) => {
            app.view_mode = match mode {
                ViewModeCmd::Text => ViewMode::Text,
                ViewModeCmd::TokenXray => ViewMode::TokenXray,
                ViewModeCmd::Tree => ViewMode::Tree,
            };
        }
        TuiCommand::ShowDetail(show) => {
            app.show_detail = show;
        }
        TuiCommand::ScrollDown(n) => {
            app.scroll_down(n);
        }
        TuiCommand::ScrollUp(n) => {
            app.scroll_up(n);
        }
        TuiCommand::GotoTop => {
            app.goto_top();
        }
        TuiCommand::GotoBottom => {
            app.goto_bottom();
        }
    }
}

/// Run dedup mode (headless, no TUI)
fn run_dedup_mode(args: &Args, dataset: &Dataset) -> Result<()> {
    let strategy = match args.dedup_strategy.as_str() {
        "exact" => DedupStrategy::Exact,
        _ => DedupStrategy::SimHash {
            threshold: args.dedup_threshold,
        },
    };

    eprintln!("Running dedup scan ({})...", strategy);
    let engine = DedupEngine::new(strategy);
    let result = engine.scan(dataset);

    eprintln!("\nDedup Results:");
    eprintln!("   {}", result.summary());

    // Export deduplicated dataset if requested
    if let Some(ref export_path) = args.dedup_export {
        eprintln!("\nExporting deduplicated dataset to {}...", export_path);

        let file = File::create(export_path)
            .with_context(|| format!("Failed to create export file: {}", export_path))?;
        let mut writer = BufWriter::new(file);

        let mut exported = 0usize;
        for i in 0..dataset.line_count() {
            if !result.is_duplicate(i) {
                if let Some(line) = dataset.get_line(i) {
                    writeln!(writer, "{}", line)?;
                    exported += 1;
                }
            }
        }

        writer.flush()?;
        eprintln!("Exported {} unique lines to {}", exported, export_path);
    }

    Ok(())
}

/// Run fix mode (headless, no TUI)
fn run_fix_mode(args: &Args, dataset: &Dataset) -> Result<()> {
    eprintln!("Running fix mode...");

    // Determine output path
    let output_path = if args.fix_in_place {
        eprintln!("WARNING: Fixing in place will overwrite the original file!");
        args.file.clone()
    } else if let Some(ref path) = args.fix_output {
        path.clone()
    } else {
        // Default: add _fixed suffix before extension
        let path = Path::new(&args.file);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let ext = path.extension().map(|e| e.to_string_lossy()).unwrap_or_default();
        if ext.is_empty() {
            format!("{}_fixed", args.file)
        } else {
            let parent = path.parent().unwrap_or(Path::new(""));
            parent.join(format!("{}_fixed.{}", stem, ext)).to_string_lossy().to_string()
        }
    };

    // For in-place fixing, write to temp file first
    let temp_path = if args.fix_in_place {
        format!("{}.tmp", output_path)
    } else {
        output_path.clone()
    };

    let fixer = Fixer::new();
    let mut summary = FixSummary::new();

    // Open output file
    let file = File::create(&temp_path)
        .with_context(|| format!("Failed to create output file: {}", temp_path))?;
    let mut writer = BufWriter::new(file);

    // Process each line
    for i in 0..dataset.line_count() {
        if let Some(line) = dataset.get_line(i) {
            match fixer.fix_line(line) {
                FixResult::Fixed { line: fixed, fixes } => {
                    writeln!(writer, "{}", fixed)?;
                    summary.record_fixed(&fixes);
                }
                FixResult::Unchanged(line) => {
                    writeln!(writer, "{}", line)?;
                    summary.record_unchanged();
                }
                FixResult::Skipped(reason) => {
                    summary.record_skipped();
                    if args.skip_invalid {
                        eprintln!("Line {}: {} (skipped)", i + 1, reason.description());
                    } else {
                        // Write original line even if invalid (preserve data)
                        writeln!(writer, "{}", line)?;
                        eprintln!("Line {}: {} (kept as-is)", i + 1, reason.description());
                    }
                }
            }
        }
    }

    writer.flush()?;

    // For in-place fixing, rename temp file to original
    if args.fix_in_place {
        std::fs::rename(&temp_path, &output_path)
            .with_context(|| format!("Failed to rename temp file to {}", output_path))?;
    }

    // Print summary
    eprintln!("\nFix Summary:");
    eprintln!("   Total lines:     {}", summary.total_lines);
    eprintln!("   Fixed lines:     {}", summary.fixed_lines);
    eprintln!("   Unchanged lines: {}", summary.unchanged_lines);
    eprintln!("   Skipped lines:   {}", summary.skipped_lines);

    if !summary.fixes_by_type.is_empty() {
        eprintln!("\nFixes applied:");
        for (fix_type, count) in &summary.fixes_by_type {
            eprintln!("   {} x{}", fix_type, count);
        }
    }

    eprintln!("\nOutput written to: {}", output_path);

    Ok(())
}
