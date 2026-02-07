//! Caret - Token X-Ray visualization
//!
//! Integrates HuggingFace tokenizers for visualizing token boundaries.

use anyhow::Result;
use lru::LruCache;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::Path;
use tokenizers::Tokenizer;

/// Color palette for token visualization
const TOKEN_COLORS: [Color; 4] = [
    Color::Rgb(70, 130, 180),  // Steel Blue
    Color::Rgb(60, 60, 60),    // Dark Gray
    Color::Rgb(100, 149, 237), // Cornflower Blue
    Color::Rgb(80, 80, 80),    // Medium Gray
];

/// Cache size for tokenized lines (avoids re-tokenizing on scroll)
const CACHE_SIZE: usize = 500;

/// Wrapper around HuggingFace tokenizer with LRU cache for performance
pub struct TokenizerWrapper {
    tokenizer: Tokenizer,
    pub name: String,
    /// LRU cache for tokenized line offsets to avoid re-encoding
    cache: RefCell<LruCache<String, Vec<(usize, usize)>>>,
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

        Ok(Self {
            tokenizer,
            name,
            cache: RefCell::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
        })
    }

    /// Load a pretrained tokenizer from HuggingFace Hub
    pub fn from_pretrained(model_id: &str) -> Result<Self> {
        let tokenizer = Tokenizer::from_pretrained(model_id, None)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer from Hub: {}", e))?;

        Ok(Self {
            tokenizer,
            name: model_id.to_string(),
            cache: RefCell::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
        })
    }

    /// Get token offsets, using cache if available
    fn get_offsets(&self, text: &str) -> Option<Vec<(usize, usize)>> {
        // Check cache first
        let cache_key = text.to_string();
        {
            let mut cache = self.cache.borrow_mut();
            if let Some(cached) = cache.get(&cache_key) {
                return Some(cached.clone());
            }
        }

        // Encode and cache the result
        let encoding = self.tokenizer.encode(text, false).ok()?;
        let offsets: Vec<(usize, usize)> = encoding.get_offsets().to_vec();
        
        {
            let mut cache = self.cache.borrow_mut();
            cache.put(cache_key, offsets.clone());
        }

        Some(offsets)
    }

    /// Tokenize text and return spans with alternating background colors
    pub fn colorize_tokens(&self, text: &str) -> Line<'static> {
        let offsets = match self.get_offsets(text) {
            Some(o) => o,
            None => return Line::from(text.to_string()),
        };

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
    #[allow(dead_code)]
    pub fn count_tokens(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_tokens().len())
            .unwrap_or(0)
    }

    /// Get token IDs for text
    #[allow(dead_code)]
    pub fn get_token_ids(&self, text: &str) -> Vec<u32> {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().to_vec())
            .unwrap_or_default()
    }
}
