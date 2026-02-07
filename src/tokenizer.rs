//! Caret - Token X-Ray visualization
//!
//! Supports multiple tokenizer backends:
//! - Tiktoken (cl100k_base, p50k_base, r50k_base) - Modern, efficient
//! - HuggingFace tokenizers (from Hub or local file)
//! - GPT-2 (legacy, via HuggingFace)

use anyhow::Result;
use lru::LruCache;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::Path;

/// Color palette for token visualization
const TOKEN_COLORS: [Color; 4] = [
    Color::Rgb(70, 130, 180),  // Steel Blue
    Color::Rgb(60, 60, 60),    // Dark Gray
    Color::Rgb(100, 149, 237), // Cornflower Blue
    Color::Rgb(80, 80, 80),    // Medium Gray
];

/// Cache size for tokenized lines (avoids re-tokenizing on scroll)
const CACHE_SIZE: usize = 500;

/// Available tokenizer types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenizerType {
    /// Tiktoken (cl100k_base by default) - Modern, efficient, used by GPT-4
    #[default]
    Tiktoken,
    /// HuggingFace tokenizers (Llama 3.1 by default)
    HuggingFace,
    /// GPT-2 tokenizer (legacy, via HuggingFace)
    Gpt2,
}

impl TokenizerType {
    /// Parse from CLI string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "tiktoken" | "tk" | "openai" => Some(TokenizerType::Tiktoken),
            "huggingface" | "hf" | "llama" => Some(TokenizerType::HuggingFace),
            "gpt2" | "gpt-2" | "legacy" => Some(TokenizerType::Gpt2),
            _ => None,
        }
    }
}

/// Tiktoken encoding types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TiktokenEncoding {
    /// cl100k_base - Used by GPT-4, GPT-3.5-turbo, text-embedding-ada-002
    #[default]
    Cl100kBase,
    /// p50k_base - Used by Codex models
    P50kBase,
    /// r50k_base - Used by GPT-3 (davinci)
    R50kBase,
}

impl TiktokenEncoding {
    /// Parse from CLI string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cl100k_base" | "cl100k" | "gpt4" => Some(TiktokenEncoding::Cl100kBase),
            "p50k_base" | "p50k" | "codex" => Some(TiktokenEncoding::P50kBase),
            "r50k_base" | "r50k" | "gpt3" => Some(TiktokenEncoding::R50kBase),
            _ => None,
        }
    }
}

/// Backend-specific tokenizer implementation
enum TokenizerBackend {
    /// Tiktoken BPE tokenizer
    Tiktoken(tiktoken_rs::CoreBPE),
    /// HuggingFace tokenizer
    HuggingFace(tokenizers::Tokenizer),
}

/// Wrapper around tokenizers with LRU cache for performance
pub struct TokenizerWrapper {
    backend: TokenizerBackend,
    pub name: String,
    /// LRU cache for tokenized line offsets to avoid re-encoding
    cache: RefCell<LruCache<String, Vec<(usize, usize)>>>,
}

impl TokenizerWrapper {
    /// Create a Tiktoken tokenizer with the specified encoding
    pub fn from_tiktoken(encoding: TiktokenEncoding) -> Result<Self> {
        let (bpe, name) = match encoding {
            TiktokenEncoding::Cl100kBase => {
                (tiktoken_rs::cl100k_base()?, "tiktoken/cl100k_base".to_string())
            }
            TiktokenEncoding::P50kBase => {
                (tiktoken_rs::p50k_base()?, "tiktoken/p50k_base".to_string())
            }
            TiktokenEncoding::R50kBase => {
                (tiktoken_rs::r50k_base()?, "tiktoken/r50k_base".to_string())
            }
        };

        Ok(Self {
            backend: TokenizerBackend::Tiktoken(bpe),
            name,
            cache: RefCell::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
        })
    }

    /// Load a tokenizer from a JSON file (HuggingFace format)
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let tokenizer = tokenizers::Tokenizer::from_file(path_ref)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let name = path_ref
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(Self {
            backend: TokenizerBackend::HuggingFace(tokenizer),
            name,
            cache: RefCell::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
        })
    }

    /// Load a pretrained tokenizer from HuggingFace Hub
    pub fn from_pretrained(model_id: &str) -> Result<Self> {
        let tokenizer = tokenizers::Tokenizer::from_pretrained(model_id, None)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer from Hub: {}", e))?;

        Ok(Self {
            backend: TokenizerBackend::HuggingFace(tokenizer),
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

        // Encode based on backend
        let offsets = match &self.backend {
            TokenizerBackend::Tiktoken(bpe) => {
                // Tiktoken doesn't provide byte offsets directly, so we need to decode each token
                // to reconstruct offsets
                let tokens = bpe.encode_with_special_tokens(text);
                let mut offsets = Vec::new();
                let mut current_pos = 0;
                
                for token_id in tokens {
                    if let Ok(token_bytes) = bpe.decode(vec![token_id]) {
                        let token_len = token_bytes.len();
                        if current_pos + token_len <= text.len() {
                            offsets.push((current_pos, current_pos + token_len));
                            current_pos += token_len;
                        }
                    }
                }
                offsets
            }
            TokenizerBackend::HuggingFace(tokenizer) => {
                let encoding = tokenizer.encode(text, false).ok()?;
                encoding.get_offsets().to_vec()
            }
        };

        // Cache the result
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
        match &self.backend {
            TokenizerBackend::Tiktoken(bpe) => {
                bpe.encode_with_special_tokens(text).len()
            }
            TokenizerBackend::HuggingFace(tokenizer) => {
                tokenizer
                    .encode(text, false)
                    .map(|e| e.get_tokens().len())
                    .unwrap_or(0)
            }
        }
    }

    /// Get token IDs for text
    #[allow(dead_code)]
    pub fn get_token_ids(&self, text: &str) -> Vec<u32> {
        match &self.backend {
            TokenizerBackend::Tiktoken(bpe) => {
                bpe.encode_with_special_tokens(text)
                    .into_iter()
                    .map(|id| id as u32)
                    .collect()
            }
            TokenizerBackend::HuggingFace(tokenizer) => {
                tokenizer
                    .encode(text, false)
                    .map(|e| e.get_ids().to_vec())
                    .unwrap_or_default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tiktoken_counting() {
        let tokenizer = TokenizerWrapper::from_tiktoken(TiktokenEncoding::Cl100kBase).unwrap();
        let count = tokenizer.count_tokens("Hello, world!");
        assert!(count > 0);
        assert!(count < 10); // Should be a few tokens
    }

    #[test]
    fn test_tokenizer_type_parsing() {
        assert_eq!(TokenizerType::from_str("tiktoken"), Some(TokenizerType::Tiktoken));
        assert_eq!(TokenizerType::from_str("huggingface"), Some(TokenizerType::HuggingFace));
        assert_eq!(TokenizerType::from_str("gpt2"), Some(TokenizerType::Gpt2));
        assert_eq!(TokenizerType::from_str("unknown"), None);
    }

    #[test]
    fn test_encoding_parsing() {
        assert_eq!(TiktokenEncoding::from_str("cl100k_base"), Some(TiktokenEncoding::Cl100kBase));
        assert_eq!(TiktokenEncoding::from_str("p50k_base"), Some(TiktokenEncoding::P50kBase));
        assert_eq!(TiktokenEncoding::from_str("r50k_base"), Some(TiktokenEncoding::R50kBase));
    }
}
