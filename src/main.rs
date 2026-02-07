//! Caret - A blazingly fast TUI for inspecting LLM datasets
//!
//! Provides instant file opening for massive JSONL files,
//! token visualization, and reasoning data validation.

mod app;
mod data;
mod linter;
mod tokenizer;
mod tui;
mod ui;

use anyhow::{Context, Result};
use argh::FromArgs;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use app::App;
use data::Dataset;
use linter::Linter;
use tokenizer::TokenizerWrapper;
use tui::Tui;

/// Caret - Blazingly fast TUI for LLM dataset curation
#[derive(FromArgs)]
struct Args {
    /// path to the JSONL file to inspect
    #[argh(positional)]
    file: String,

    /// path to tokenizer.json for Token X-Ray mode
    #[argh(option, short = 't')]
    tokenizer: Option<String>,

    /// run the linter on load
    #[argh(switch, short = 'l')]
    lint: bool,

    /// required keys to check for (comma-separated)
    #[argh(option, short = 'k')]
    required_keys: Option<String>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    // Load the dataset - support stdin with "-"
    let dataset = if args.file == "-" {
        eprintln!("📂 Reading from stdin...");
        let dataset = Dataset::from_stdin()
            .with_context(|| "Failed to read from stdin")?;
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

    // Load tokenizer if provided
    if let Some(tokenizer_path) = args.tokenizer {
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
    }

    // Run linter if requested
    if args.lint {
        eprintln!("🔍 Running linter...");
        let mut linter = Linter::new();
        if let Some(keys) = args.required_keys {
            let keys: Vec<String> = keys.split(',').map(|s| s.trim().to_string()).collect();
            linter = linter.with_required_keys(keys);
        }
        let results = linter.lint_dataset(&app.dataset);
        eprintln!("✓ Found {} issues", results.len());
        app = app.with_lint_results(results);
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
