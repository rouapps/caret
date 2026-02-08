//! Caret - Application state management

use crate::data::Dataset;
use crate::engine::{DedupEngine, DedupResult, DedupStrategy};
use crate::linter::LintResult;
use crate::tokenizer::TokenizerWrapper;

/// View mode for the main display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Normal text view with syntax highlighting
    Text,
    /// Token X-Ray mode showing tokenization boundaries
    TokenXray,
    /// JSON tree view for nested structures
    Tree,
}

impl ViewMode {
    pub fn toggle(&mut self) {
        *self = match self {
            ViewMode::Text => ViewMode::TokenXray,
            ViewMode::TokenXray => ViewMode::Tree,
            ViewMode::Tree => ViewMode::Text,
        };
    }

    pub fn label(&self) -> &'static str {
        match self {
            ViewMode::Text => "TEXT",
            ViewMode::TokenXray => "TOKEN X-RAY",
            ViewMode::Tree => "TREE",
        }
    }
}

/// Main application state
pub struct App {
    /// The loaded dataset
    pub dataset: Dataset,
    /// Current scroll position (line index)
    pub scroll: usize,
    /// Number of visible lines in the viewport
    pub viewport_height: usize,
    /// Current view mode
    pub view_mode: ViewMode,
    /// Optional tokenizer for X-Ray mode
    pub tokenizer: Option<TokenizerWrapper>,
    /// Lint results for the current dataset
    pub lint_results: Vec<LintResult>,
    /// Deduplication scan results (None if no scan has been run)
    pub dedup_result: Option<DedupResult>,
    /// Whether to show the help popup
    pub show_help: bool,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Currently selected line for details
    pub selected_line: usize,
    /// Whether to show the detail panel
    pub show_detail: bool,
    /// Tree expansion state for JSON tree view
    #[allow(dead_code)]
    pub tree_expanded: std::collections::HashSet<String>,
}

impl App {
    /// Create a new app with the given dataset
    pub fn new(dataset: Dataset) -> Self {
        Self {
            dataset,
            scroll: 0,
            viewport_height: 20,
            view_mode: ViewMode::Text,
            tokenizer: None,
            lint_results: Vec::new(),
            dedup_result: None,
            show_help: false,
            should_quit: false,
            selected_line: 0,
            show_detail: false,
            tree_expanded: std::collections::HashSet::new(),
        }
    }

    /// Toggle detail panel visibility
    pub fn toggle_detail(&mut self) {
        self.show_detail = !self.show_detail;
    }

    /// Toggle dedup scan: run if no result, clear if already scanned.
    pub fn toggle_dedup(&mut self) {
        if self.dedup_result.is_some() {
            self.dedup_result = None;
        } else {
            let engine = DedupEngine::new(DedupStrategy::SimHash { threshold: 3 });
            let result = engine.scan(&self.dataset);
            self.dedup_result = Some(result);
        }
    }

    /// Get the current line content
    pub fn current_line_content(&self) -> Option<&str> {
        self.dataset.get_line(self.selected_line)
    }

    /// Get pretty-printed JSON for current line
    pub fn current_line_pretty(&self) -> String {
        if let Some(line) = self.current_line_content() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| line.to_string())
            } else {
                line.to_string()
            }
        } else {
            String::new()
        }
    }

    /// Set the tokenizer for X-Ray mode
    pub fn with_tokenizer(mut self, tokenizer: TokenizerWrapper) -> Self {
        self.tokenizer = Some(tokenizer);
        self
    }

    /// Set lint results
    pub fn with_lint_results(mut self, results: Vec<LintResult>) -> Self {
        self.lint_results = results;
        self
    }

    /// Scroll down by n lines
    pub fn scroll_down(&mut self, n: usize) {
        let max_scroll = self
            .dataset
            .line_count()
            .saturating_sub(self.viewport_height);
        self.scroll = (self.scroll + n).min(max_scroll);
        self.selected_line =
            (self.selected_line + n).min(self.dataset.line_count().saturating_sub(1));
    }

    /// Scroll up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
        self.selected_line = self.selected_line.saturating_sub(n);
    }

    /// Jump to the beginning
    pub fn goto_top(&mut self) {
        self.scroll = 0;
        self.selected_line = 0;
    }

    /// Jump to the end
    pub fn goto_bottom(&mut self) {
        let max_scroll = self
            .dataset
            .line_count()
            .saturating_sub(self.viewport_height);
        self.scroll = max_scroll;
        self.selected_line = self.dataset.line_count().saturating_sub(1);
    }

    /// Update viewport height based on terminal size
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.saturating_sub(4); // Account for borders and status bar
    }

    /// Check if a line has lint errors
    pub fn line_has_error(&self, line_index: usize) -> bool {
        self.lint_results.iter().any(|r| r.line == line_index)
    }

    /// Check if a line is a duplicate (per dedup scan)
    pub fn line_is_duplicate(&self, line_index: usize) -> bool {
        self.dedup_result
            .as_ref()
            .map(|r| r.is_duplicate(line_index))
            .unwrap_or(false)
    }

    /// Get lint error for a specific line
    #[allow(dead_code)]
    pub fn get_lint_error(&self, line_index: usize) -> Option<&LintResult> {
        self.lint_results.iter().find(|r| r.line == line_index)
    }
}
