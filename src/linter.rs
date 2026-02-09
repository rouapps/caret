//! Caret - Reasoning dataset linter
//!
//! Validates Chain-of-Thought datasets for common errors.

use crate::data::Dataset;
use regex::Regex;

/// Types of lint errors
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LintError {
    /// Invalid JSON structure
    InvalidJson(String),
    /// Missing required key
    MissingKey(String),
    /// Unbalanced think tags
    UnbalancedThinkTags { open: usize, close: usize },
    /// Trailing whitespace before important tokens
    TrailingWhitespace,
    /// Empty content
    EmptyContent,
}

#[allow(dead_code)]
impl LintError {
    pub fn message(&self) -> String {
        match self {
            LintError::InvalidJson(e) => format!("Invalid JSON: {}", e),
            LintError::MissingKey(key) => format!("Missing required key: {}", key),
            LintError::UnbalancedThinkTags { open, close } => {
                format!("Unbalanced <think> tags: {} open, {} close", open, close)
            }
            LintError::TrailingWhitespace => "Trailing whitespace detected".to_string(),
            LintError::EmptyContent => "Empty content field".to_string(),
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            LintError::InvalidJson(_) => "ERROR",
            LintError::UnbalancedThinkTags { .. } => "ERROR",
            LintError::MissingKey(_) => "WARNING",
            LintError::TrailingWhitespace => "WARNING",
            LintError::EmptyContent => "WARNING",
        }
    }
}

/// Result of linting a single line
#[derive(Debug, Clone)]
pub struct LintResult {
    /// Line number (0-indexed)
    pub line: usize,
    /// The error found
    #[allow(dead_code)]
    pub error: LintError,
}

/// Linter for reasoning datasets
pub struct Linter {
    think_open_regex: Regex,
    think_close_regex: Regex,
    required_keys: Vec<String>,
}

impl Default for Linter {
    fn default() -> Self {
        Self::new()
    }
}

impl Linter {
    /// Create a new linter with default settings
    pub fn new() -> Self {
        Self {
            think_open_regex: Regex::new(r"<think>").expect("valid regex: <think>"),
            think_close_regex: Regex::new(r"</think>").expect("valid regex: </think>"),
            required_keys: vec![],
        }
    }

    /// Add required keys to check for
    pub fn with_required_keys(mut self, keys: Vec<String>) -> Self {
        self.required_keys = keys;
        self
    }

    /// Lint a single line of text
    pub fn lint_line(&self, line: &str, line_num: usize) -> Vec<LintResult> {
        let mut results = Vec::new();

        // Check JSON validity
        let json_value: Result<serde_json::Value, _> = serde_json::from_str(line);
        match json_value {
            Err(e) => {
                results.push(LintResult {
                    line: line_num,
                    error: LintError::InvalidJson(e.to_string()),
                });
                return results; // Can't do further checks on invalid JSON
            }
            Ok(value) => {
                // Check for required keys
                if let Some(obj) = value.as_object() {
                    for key in &self.required_keys {
                        if !obj.contains_key(key) {
                            results.push(LintResult {
                                line: line_num,
                                error: LintError::MissingKey(key.clone()),
                            });
                        }
                    }
                }

                // Check for balanced think tags in string values
                let text_content = extract_text_content(&value);
                let open_count = self.think_open_regex.find_iter(&text_content).count();
                let close_count = self.think_close_regex.find_iter(&text_content).count();

                if open_count != close_count {
                    results.push(LintResult {
                        line: line_num,
                        error: LintError::UnbalancedThinkTags {
                            open: open_count,
                            close: close_count,
                        },
                    });
                }

                // Check for trailing whitespace before common stop tokens
                if text_content.contains(" \n") || text_content.ends_with(' ') {
                    results.push(LintResult {
                        line: line_num,
                        error: LintError::TrailingWhitespace,
                    });
                }
            }
        }

        results
    }

    /// Lint entire dataset
    pub fn lint_dataset(&self, dataset: &Dataset) -> Vec<LintResult> {
        let mut all_results = Vec::new();

        for i in 0..dataset.line_count() {
            if let Some(line) = dataset.get_line(i) {
                if !line.trim().is_empty() {
                    let results = self.lint_line(line, i);
                    all_results.extend(results);
                }
            }
        }

        all_results
    }
}

/// Extract all text content from a JSON value for analysis
fn extract_text_content(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(extract_text_content)
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::Object(obj) => obj
            .values()
            .map(extract_text_content)
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balanced_think_tags() {
        let linter = Linter::new();
        let good = r#"{"text": "<think>reasoning</think>answer"}"#;
        let bad = r#"{"text": "<think>reasoning answer"}"#;

        assert!(linter.lint_line(good, 0).is_empty());
        assert!(!linter.lint_line(bad, 0).is_empty());
    }

    #[test]
    fn test_invalid_json() {
        let linter = Linter::new();
        let results = linter.lint_line("not json {", 0);
        assert!(matches!(results[0].error, LintError::InvalidJson(_)));
    }
}
