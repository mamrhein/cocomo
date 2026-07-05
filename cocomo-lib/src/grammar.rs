// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Grammar system for syntax-aware text comparison.
//!
//! Grammars define rules that classify text tokens and lines into importance
//! levels (code, data, comment, ignored). This enables smart diffing that can
//! skip unimportant changes, normalize variations, and drive
//! "next difference" navigation in the UI.

use std::fmt;

use regex::Regex;

/// Importance level assigned to a text token or line by a grammar rule.
///
/// Drives "next difference" navigation and display filtering:
/// - **Code** — high importance, always shown.
/// - **Data** — medium importance.
/// - **Comment** — low importance, can be hidden.
/// - **Ignored** — not shown unless filters are suppressed.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub enum Importance {
    /// Not shown in diffs unless display filters are suppressed.
    #[default]
    Ignored,
    /// Low importance; comments, documentation.
    Comment,
    /// Medium importance; data, configuration values, literals.
    Data,
    /// High importance; executable code or structural elements.
    Code,
}

impl fmt::Display for Importance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Importance::Code => write!(f, "code"),
            Importance::Data => write!(f, "data"),
            Importance::Comment => write!(f, "comment"),
            Importance::Ignored => write!(f, "ignored"),
        }
    }
}

/// A single grammar rule that matches a regex pattern and assigns an
/// importance level to matching text.
#[derive(Clone, Debug)]
pub struct GrammarRule {
    /// Descriptive name for this rule (e.g., "line_comment",
    /// "string_literal").
    pub name: String,
    /// Compiled regex pattern used to match text.
    pub pattern: Regex,
    /// Importance assigned to text that matches this rule.
    pub importance: Importance,
    /// Optional replacement template to normalize matching text. Used during
    /// comparison so that equivalent variations are treated as equal.
    pub replacement: Option<String>,
}

impl GrammarRule {
    /// Create a new grammar rule.
    pub fn new(
        name: impl Into<String>,
        pattern: Regex,
        importance: Importance,
        replacement: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            pattern,
            importance,
            replacement,
        }
    }

    /// Apply the replacement template to matched text, returning normalized
    /// output. Returns the original text unchanged when no replacement is set.
    pub fn normalize(&self, text: &str) -> String {
        if let Some(ref repl) = self.replacement {
            self.pattern.replace_all(text, repl.as_str()).to_string()
        } else {
            text.to_string()
        }
    }
}

/// A named collection of grammar rules for a specific programming language or
/// file format.
///
/// Grammars enable syntax highlighting, smart diffing (ignoring comments,
/// strings, whitespace-only changes), text normalization, and line
/// classification that drives "next difference" navigation.
#[derive(Clone, Debug, Default)]
pub struct Grammar {
    /// Human-readable name of the grammar (e.g., "Rust", "Python").
    pub name: String,
    /// Ordered list of rules. Rules are evaluated in order; the first match
    /// wins.
    pub rules: Vec<GrammarRule>,
}

impl Grammar {
    /// Create a new empty grammar with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rules: Vec::new(),
        }
    }

    /// Add a rule to this grammar.
    pub fn add_rule(
        &mut self,
        name: impl Into<String>,
        pattern: Regex,
        importance: Importance,
        replacement: Option<String>,
    ) {
        self.rules.push(GrammarRule::new(
            name,
            pattern,
            importance,
            replacement,
        ));
    }

    /// Classify a line of text by running it against the grammar rules.
    /// Returns the importance of the first matching rule, or
    /// `Importance::Code` when no rule matches.
    pub fn classify_line(&self, line: &str) -> Importance {
        for rule in &self.rules {
            if rule.pattern.is_match(line) {
                return rule.importance;
            }
        }
        Importance::Code
    }

    /// Normalize a line of text by applying all matching rule replacements.
    /// Returns the transformed text.
    pub fn normalize_line(&self, line: &str) -> String {
        let mut result = line.to_string();
        for rule in &self.rules {
            if rule.replacement.is_some() && rule.pattern.is_match(&result) {
                result = rule.normalize(&result);
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Built-in grammars for common languages
// ---------------------------------------------------------------------------

impl Grammar {
    /// Return a built-in grammar for Rust source files.
    pub fn rust() -> Self {
        let mut g = Self::new("Rust");
        g.add_rule(
            "line_comment",
            Regex::new(r"^\s*//").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "block_comment_start",
            Regex::new(r"^\s*/\*").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "block_comment_inner",
            Regex::new(r"^\s*\*").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "doc_comment",
            Regex::new(r"^\s*///|^\s*/\*\*").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "string_literal",
            Regex::new(r#"^\s*"[^"]*""#).unwrap(),
            Importance::Data,
            None,
        );
        g.add_rule(
            "trailing_whitespace",
            Regex::new(r"\s+$").unwrap(),
            Importance::Ignored,
            Some("".into()),
        );
        g
    }

    /// Return a built-in grammar for Python source files.
    pub fn python() -> Self {
        let mut g = Self::new("Python");
        g.add_rule(
            "line_comment",
            Regex::new(r"^\s*#").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "trailing_whitespace",
            Regex::new(r"\s+$").unwrap(),
            Importance::Ignored,
            Some("".into()),
        );
        g
    }

    /// Return a built-in grammar for C/C++ source files.
    pub fn c() -> Self {
        let mut g = Self::new("C/C++");
        g.add_rule(
            "line_comment",
            Regex::new(r"^\s*//").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "block_comment",
            Regex::new(r"^\s*/\*|^\s*\*|^\s*\*/").unwrap(),
            Importance::Comment,
            None,
        );
        g.add_rule(
            "preprocessor",
            Regex::new(r"^\s*#(include|define|ifdef|ifndef|endif|if|else)")
                .unwrap(),
            Importance::Code,
            None,
        );
        g.add_rule(
            "trailing_whitespace",
            Regex::new(r"\s+$").unwrap(),
            Importance::Ignored,
            Some("".into()),
        );
        g
    }

    /// Return a built-in grammar for generic text/plain files.
    pub fn plain_text() -> Self {
        let mut g = Self::new("Plain Text");
        g.add_rule(
            "trailing_whitespace",
            Regex::new(r"\s+$").unwrap(),
            Importance::Ignored,
            Some("".into()),
        );
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_display() {
        assert_eq!(format!("{}", Importance::Code), "code");
        assert_eq!(format!("{}", Importance::Comment), "comment");
        assert_eq!(format!("{}", Importance::Ignored), "ignored");
    }

    #[test]
    fn classify_rust_comment() {
        let g = Grammar::rust();
        assert_eq!(
            g.classify_line("// this is a comment"),
            Importance::Comment
        );
        assert_eq!(
            g.classify_line("    /// doc comment"),
            Importance::Comment
        );
    }

    #[test]
    fn classify_rust_code() {
        let g = Grammar::rust();
        assert_eq!(g.classify_line("fn main() {}"), Importance::Code);
        assert_eq!(g.classify_line("let x = 42;"), Importance::Code);
    }

    #[test]
    fn normalize_trailing_whitespace() {
        let g = Grammar::rust();
        let normalized = g.normalize_line("let x = 42;   ");
        assert_eq!(normalized, "let x = 42;");
    }

    #[test]
    fn python_comment_classification() {
        let g = Grammar::python();
        assert_eq!(g.classify_line("# comment"), Importance::Comment);
        assert_eq!(g.classify_line("def foo():"), Importance::Code);
    }

    #[test]
    fn empty_grammar_defaults_to_code() {
        let g = Grammar::new("empty");
        assert_eq!(g.classify_line("anything"), Importance::Code);
    }

    #[test]
    fn grammar_rule_normalize_no_replacement() {
        let rule = GrammarRule::new(
            "test",
            Regex::new(r"\s+").unwrap(),
            Importance::Code,
            None,
        );
        assert_eq!(rule.normalize("hello  world"), "hello  world");
    }

    #[test]
    fn grammar_rule_normalize_with_replacement() {
        let rule = GrammarRule::new(
            "collapse_spaces",
            Regex::new(r"\s+").unwrap(),
            Importance::Ignored,
            Some(" ".into()),
        );
        assert_eq!(rule.normalize("hello   world"), "hello world");
    }
}
