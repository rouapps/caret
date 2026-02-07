//! Caret - A blazingly fast TUI for inspecting LLM datasets
//!
//! Provides instant file opening for massive JSONL files,
//! token visualization, and reasoning data validation.

mod app;
mod data;
mod fixer;
mod linter;
mod tokenizer;
mod tui;
mod ui;

use anyhow::{Context, Result};
use argh::FromArgs;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use app::App;
use data::Dataset;
use fixer::{FixResult, Fixer, FixSummary};
use linter::Linter;
use tokenizer::TokenizerWrapper;
use tui::Tui;

/// Caret - Blazingly fast TUI for LLM dataset curation
#[derive(FromArgs)]
struct Args {
    /// path to the JSONL file to inspect
    #[argh(positional)]
    file: String,

    /// enable Token X-Ray mode with default Llama 3.1 tokenizer
    #[argh(switch, short = 't')]
    tokenizer: bool,

    /// path to custom tokenizer.json (overrides default)
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
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    // Load the dataset - support stdin with "-"
    let dataset = if args.file == "-" {
        eprintln!("📂 Reading from stdin...");
        let dataset = Dataset::from_stdin().with_context(|| "Failed to read from stdin")?;
        eprintln!(
            "✓ Loaded {} lines ({}) from stdin",
            dataset.line_count(),
            dataset.size_human()
        );
        dataset
    } else {
        eprintln!("📂 Opening {}...", args.file);
        let dataset = Dataset::open(&args.file)
            .with_context(|| format!("Failed to open dataset: {}", args.file))?;
        eprintln!(
            "✓ Loaded {} lines ({}) in memory-mapped mode",
            dataset.line_count(),
            dataset.size_human()
        );
        dataset
    };

    // Create the app
    let mut app = App::new(dataset);

    // Load tokenizer if requested
    if let Some(ref tokenizer_path) = args.tokenizer_path {
        // Custom tokenizer path takes priority
        eprintln!("🔤 Loading tokenizer from {}...", tokenizer_path);
        match TokenizerWrapper::from_file(&tokenizer_path) {
            Ok(tokenizer) => {
                eprintln!("✓ Tokenizer loaded: {}", tokenizer.name);
                app = app.with_tokenizer(tokenizer);
            }
            Err(e) => {
                eprintln!("⚠ Failed to load tokenizer: {}", e);
            }
        }
    } else if args.tokenizer {
        // Use default Llama 3.1 tokenizer from HuggingFace Hub
        eprintln!("🔤 Loading default tokenizer (Llama 3.1) from HuggingFace...");
        match TokenizerWrapper::from_pretrained("meta-llama/Llama-3.1-8B") {
            Ok(tokenizer) => {
                eprintln!("✓ Tokenizer loaded: {}", tokenizer.name);
                app = app.with_tokenizer(tokenizer);
            }
            Err(e) => {
                eprintln!("⚠ Failed to load tokenizer: {}", e);
                eprintln!("  Tip: You may need to accept Llama 3.1's license on HuggingFace");
                eprintln!("  Falling back to GPT-2...");
                // Fallback to GPT-2 which doesn't require authentication
                match TokenizerWrapper::from_pretrained("gpt2") {
                    Ok(tokenizer) => {
                        eprintln!("✓ Tokenizer loaded: {}", tokenizer.name);
                        app = app.with_tokenizer(tokenizer);
                    }
                    Err(e) => {
                        eprintln!("⚠ Failed to load fallback tokenizer: {}", e);
                    }
                }
            }
        }
    }

    // Run linter if requested
    if args.lint {
        eprintln!("🔍 Running linter...");
        let mut linter = Linter::new();
        if let Some(ref keys) = args.required_keys {
            let keys: Vec<String> = keys.split(',').map(|s| s.trim().to_string()).collect();
            linter = linter.with_required_keys(keys);
        }
        let results = linter.lint_dataset(&app.dataset);
        eprintln!("✓ Found {} issues", results.len());
        app = app.with_lint_results(results);
    }

    // Run fix mode if requested (headless, no TUI)
    if args.fix {
        return run_fix_mode(&args, &app.dataset);
    }

    // Initialize TUI
    let mut tui = Tui::new()?;

    // Main event loop
    loop {
        // Render
        tui.terminal().draw(|frame| ui::render(frame, &mut app))?;

        // Handle events
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
                        app.view_mode.toggle();
                    }

                    // Toggle detail panel
                    (KeyCode::Enter, _) => {
                        app.toggle_detail();
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
    eprintln!("👋 Goodbye!");

    Ok(())
}

/// Run fix mode (headless, no TUI)
fn run_fix_mode(args: &Args, dataset: &Dataset) -> Result<()> {
    eprintln!("🔧 Running fix mode...");

    // Determine output path
    let output_path = if args.fix_in_place {
        eprintln!("⚠️  WARNING: Fixing in place will overwrite the original file!");
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
                        eprintln!("⚠ Line {}: {} (skipped)", i + 1, reason.description());
                    } else {
                        // Write original line even if invalid (preserve data)
                        writeln!(writer, "{}", line)?;
                        eprintln!("⚠ Line {}: {} (kept as-is)", i + 1, reason.description());
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
    eprintln!("\n📊 Fix Summary:");
    eprintln!("   Total lines:     {}", summary.total_lines);
    eprintln!("   Fixed lines:     {}", summary.fixed_lines);
    eprintln!("   Unchanged lines: {}", summary.unchanged_lines);
    eprintln!("   Skipped lines:   {}", summary.skipped_lines);

    if !summary.fixes_by_type.is_empty() {
        eprintln!("\n📝 Fixes applied:");
        for (fix_type, count) in &summary.fixes_by_type {
            eprintln!("   {} x{}", fix_type, count);
        }
    }

    eprintln!("\n✅ Output written to: {}", output_path);

    Ok(())
}
