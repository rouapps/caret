//! Caret - Token X-Ray visualization
//!
//! Integrates HuggingFace tokenizers for visualizing token boundaries.

use anyhow::Result;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::path::Path;
use tokenizers::Tokenizer;

/// Color palette for token visualization
const TOKEN_COLORS: [Color; 4] = [
    Color::Rgb(70, 130, 180),  // Steel Blue
    Color::Rgb(60, 60, 60),    // Dark Gray
    Color::Rgb(100, 149, 237), // Cornflower Blue
    Color::Rgb(80, 80, 80),    // Medium Gray
];

/// Wrapper around HuggingFace tokenizer
pub struct TokenizerWrapper {
    tokenizer: Tokenizer,
    pub name: String,
}

impl TokenizerWrapper {
    /// Load a tokenizer from a JSON file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let tokenizer = Tokenizer::from_file(path_ref)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let name = path_ref
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(Self { tokenizer, name })
    }

    /// Tokenize text and return spans with alternating background colors
    pub fn colorize_tokens(&self, text: &str) -> Line<'static> {
        // Encode the text to get token offsets
        let encoding = match self.tokenizer.encode(text, false) {
            Ok(enc) => enc,
            Err(_) => return Line::from(text.to_string()),
        };

        let offsets = encoding.get_offsets();

        if offsets.is_empty() {
            return Line::from(text.to_string());
        }

        let mut spans = Vec::new();
        let mut last_end = 0;

        for (i, &(start, end)) in offsets.iter().enumerate() {
            // Add any gap between tokens as plain text
            if start > last_end {
                if let Some(gap) = text.get(last_end..start) {
                    spans.push(Span::raw(gap.to_string()));
                }
            }

            // Add the token with colored background
            if let Some(token_text) = text.get(start..end) {
                let color = TOKEN_COLORS[i % TOKEN_COLORS.len()];
                spans.push(Span::styled(
                    token_text.to_string(),
                    Style::default().bg(color).fg(Color::White),
                ));
            }

            last_end = end;
        }

        // Add any remaining text after the last token
        if last_end < text.len() {
            if let Some(remainder) = text.get(last_end..) {
                spans.push(Span::raw(remainder.to_string()));
            }
        }

        Line::from(spans)
    }

    /// Count tokens in text
    pub fn count_tokens(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_tokens().len())
            .unwrap_or(0)
    }

    /// Get token IDs for text
    pub fn get_token_ids(&self, text: &str) -> Vec<u32> {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().to_vec())
            .unwrap_or_default()
    }
}
