//! Caret - Dataset fixer
//!
//! Automatically repairs common issues in LLM training datasets.

use regex::Regex;
use serde_json::{Map, Value};

/// Types of fixes that can be applied
#[derive(Debug, Clone, PartialEq)]
pub enum FixType {
    /// Added missing </think> tag
    AddedClosingThinkTag,
    /// Added missing <think> tag
    AddedOpeningThinkTag,
    /// Removed trailing whitespace
    RemovedTrailingWhitespace,
    /// Trimmed whitespace before newlines
    TrimmedWhitespaceBeforeNewlines,
}

impl FixType {
    pub fn description(&self) -> &'static str {
        match self {
            FixType::AddedClosingThinkTag => "Added missing </think> tag",
            FixType::AddedOpeningThinkTag => "Added missing <think> tag",
            FixType::RemovedTrailingWhitespace => "Removed trailing whitespace",
            FixType::TrimmedWhitespaceBeforeNewlines => "Trimmed whitespace before newlines",
        }
    }
}

/// Reason why a line was skipped
#[derive(Debug, Clone)]
pub enum SkipReason {
    /// Invalid JSON that cannot be parsed
    InvalidJson(String),
    /// Empty line
    EmptyLine,
}

impl SkipReason {
    pub fn description(&self) -> String {
        match self {
            SkipReason::InvalidJson(e) => format!("Invalid JSON: {}", e),
            SkipReason::EmptyLine => "Empty line".to_string(),
        }
    }
}

/// Result of attempting to fix a line
#[derive(Debug)]
pub enum FixResult {
    /// Line was fixed, contains the fixed JSON string and list of fixes applied
    Fixed {
        line: String,
        fixes: Vec<FixType>,
    },
    /// Line was already valid, no fixes needed
    Unchanged(String),
    /// Line was skipped (cannot be fixed)
    Skipped(SkipReason),
}

/// Fixer for reasoning datasets
pub struct Fixer {
    think_open_regex: Regex,
    think_close_regex: Regex,
    whitespace_before_newline: Regex,
}

impl Default for Fixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixer {
    /// Create a new fixer
    pub fn new() -> Self {
        Self {
            think_open_regex: Regex::new(r"<think>").expect("valid regex: <think>"),
            think_close_regex: Regex::new(r"</think>").expect("valid regex: </think>"),
            whitespace_before_newline: Regex::new(r" +\n").expect("valid regex: whitespace before newline"),
        }
    }

    /// Fix a single line of JSONL
    pub fn fix_line(&self, line: &str) -> FixResult {
        let trimmed = line.trim();
        
        // Skip empty lines
        if trimmed.is_empty() {
            return FixResult::Skipped(SkipReason::EmptyLine);
        }

        // Try to parse as JSON
        let mut json_value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => return FixResult::Skipped(SkipReason::InvalidJson(e.to_string())),
        };

        let mut fixes = Vec::new();

        // Fix the JSON value in place
        self.fix_value(&mut json_value, &mut fixes);

        // Serialize back to JSON
        let fixed_line = serde_json::to_string(&json_value).expect("parsed JSON should be re-serializable");

        if fixes.is_empty() {
            FixResult::Unchanged(fixed_line)
        } else {
            FixResult::Fixed {
                line: fixed_line,
                fixes,
            }
        }
    }

    /// Recursively fix a JSON value
    fn fix_value(&self, value: &mut Value, fixes: &mut Vec<FixType>) {
        match value {
            Value::String(s) => {
                self.fix_string(s, fixes);
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.fix_value(item, fixes);
                }
            }
            Value::Object(obj) => {
                self.fix_object(obj, fixes);
            }
            _ => {}
        }
    }

    /// Fix a JSON object, with special handling for message objects
    fn fix_object(&self, obj: &mut Map<String, Value>, fixes: &mut Vec<FixType>) {
        // Check if this is a message object with role=assistant
        let is_assistant = obj
            .get("role")
            .and_then(|v| v.as_str())
            .map(|s| s == "assistant")
            .unwrap_or(false);

        for (key, value) in obj.iter_mut() {
            // For assistant messages, apply think tag fixes to content
            if is_assistant && key == "content" {
                if let Value::String(s) = value {
                    self.fix_string(s, fixes);
                    self.fix_think_tags(s, fixes);
                }
            } else {
                self.fix_value(value, fixes);
            }
        }
    }

    /// Fix common string issues (whitespace)
    fn fix_string(&self, s: &mut String, fixes: &mut Vec<FixType>) {
        // Fix trailing whitespace
        let original_len = s.len();
        let trimmed = s.trim_end().to_string();
        if trimmed.len() < original_len {
            *s = trimmed;
            if !fixes.contains(&FixType::RemovedTrailingWhitespace) {
                fixes.push(FixType::RemovedTrailingWhitespace);
            }
        }

        // Fix whitespace before newlines
        if self.whitespace_before_newline.is_match(s) {
            *s = self.whitespace_before_newline.replace_all(s, "\n").to_string();
            if !fixes.contains(&FixType::TrimmedWhitespaceBeforeNewlines) {
                fixes.push(FixType::TrimmedWhitespaceBeforeNewlines);
            }
        }
    }

    /// Fix unbalanced think tags
    fn fix_think_tags(&self, s: &mut String, fixes: &mut Vec<FixType>) {
        let open_count = self.think_open_regex.find_iter(s).count();
        let close_count = self.think_close_regex.find_iter(s).count();

        match open_count.cmp(&close_count) {
            std::cmp::Ordering::Greater => {
                // Missing closing tags - find where each <think> ends and add </think> if missing
                // Simple approach: add missing </think> tags at the end of each unclosed section
                for _ in 0..(open_count - close_count) {
                    // Find the last <think> that doesn't have a matching </think>
                    // For simplicity, append </think> right after the last unclosed <think>'s content
                    // A smarter approach would find where the thinking ends, but we'll use a heuristic:
                    // Insert </think> before the final answer (after all thinking is done)
                    
                    if let Some(last_open_pos) = s.rfind("<think>") {
                        // Check if there's a </think> after this position
                        let after_open = &s[last_open_pos..];
                        if !after_open.contains("</think>") {
                            // No closing tag after this opening - add one
                            // Try to find a natural break point (end of thinking)
                            // If the content has a clear answer section, insert before it
                            // Otherwise, look for patterns like double newlines
                            let close_pos = find_think_close_position(&s[last_open_pos + 7..]);
                            let insert_pos = last_open_pos + 7 + close_pos;
                            s.insert_str(insert_pos, "</think>");
                            if !fixes.contains(&FixType::AddedClosingThinkTag) {
                                fixes.push(FixType::AddedClosingThinkTag);
                            }
                        }
                    }
                }
            }
            std::cmp::Ordering::Less => {
                // Missing opening tags - prepend <think> for each unmatched </think>
                for _ in 0..(close_count - open_count) {
                    // Prepend <think> at the beginning
                    *s = format!("<think>{}", s);
                    if !fixes.contains(&FixType::AddedOpeningThinkTag) {
                        fixes.push(FixType::AddedOpeningThinkTag);
                    }
                }
            }
            std::cmp::Ordering::Equal => {}
        }
    }
}

/// Heuristic to find where to insert </think> tag
/// Returns the position relative to the start of content after <think>
fn find_think_close_position(content: &str) -> usize {
    // Look for patterns that indicate end of thinking:
    // 1. Double newline
    // 2. End of string
    
    // Check for double newline (paragraph break)
    if let Some(pos) = content.find("\n\n") {
        return pos;
    }
    
    // Default: insert at end of content
    content.len()
}

/// Summary of fixes applied to a dataset
#[derive(Debug, Default)]
pub struct FixSummary {
    pub total_lines: usize,
    pub fixed_lines: usize,
    pub unchanged_lines: usize,
    pub skipped_lines: usize,
    pub fixes_by_type: std::collections::HashMap<String, usize>,
}

impl FixSummary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_fixed(&mut self, fixes: &[FixType]) {
        self.total_lines += 1;
        self.fixed_lines += 1;
        for fix in fixes {
            *self.fixes_by_type.entry(fix.description().to_string()).or_insert(0) += 1;
        }
    }

    pub fn record_unchanged(&mut self) {
        self.total_lines += 1;
        self.unchanged_lines += 1;
    }

    pub fn record_skipped(&mut self) {
        self.total_lines += 1;
        self.skipped_lines += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_trailing_whitespace() {
        let fixer = Fixer::new();
        let input = r#"{"content": "hello world   "}"#;
        
        match fixer.fix_line(input) {
            FixResult::Fixed { line, fixes } => {
                assert!(line.contains(r#""hello world""#));
                assert!(fixes.contains(&FixType::RemovedTrailingWhitespace));
            }
            _ => panic!("Expected Fixed result"),
        }
    }

    #[test]
    fn test_fix_unclosed_think_tag() {
        let fixer = Fixer::new();
        let input = r#"{"messages": [{"role": "assistant", "content": "<think>thinking here"}]}"#;
        
        match fixer.fix_line(input) {
            FixResult::Fixed { line, fixes } => {
                assert!(line.contains("</think>"));
                assert!(fixes.contains(&FixType::AddedClosingThinkTag));
            }
            _ => panic!("Expected Fixed result"),
        }
    }

    #[test]
    fn test_unchanged_valid_line() {
        let fixer = Fixer::new();
        let input = r#"{"messages": [{"role": "assistant", "content": "<think>ok</think>answer"}]}"#;
        
        match fixer.fix_line(input) {
            FixResult::Unchanged(_) => {}
            _ => panic!("Expected Unchanged result"),
        }
    }

    #[test]
    fn test_skip_invalid_json() {
        let fixer = Fixer::new();
        let input = r#"{"broken json"#;
        
        match fixer.fix_line(input) {
            FixResult::Skipped(SkipReason::InvalidJson(_)) => {}
            _ => panic!("Expected Skipped result"),
        }
    }

    #[test]
    fn test_skip_empty_line() {
        let fixer = Fixer::new();
        
        match fixer.fix_line("") {
            FixResult::Skipped(SkipReason::EmptyLine) => {}
            _ => panic!("Expected Skipped result"),
        }
    }
}
