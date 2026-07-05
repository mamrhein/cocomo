// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! File format registry and format-specific settings.
//!
//! Each `FileFormat` describes how to handle a family of files (by extension
//! or MIME type), including the comparison mode, encoding defaults, and an
//! optional grammar for syntax-aware diffing.

use std::collections::HashMap;

use crate::grammar::Grammar;

/// The kind of comparison a file format supports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatType {
    /// Plain or syntax-highlighted text.
    Text,
    /// Tabular data (CSV, TSV, etc.). The parser field specifies the
    /// delimiter and quoting rules.
    Table { parser: TableParser },
    /// Raw binary displayed as hexadecimal.
    Hex,
    /// Image files compared pixel-wise.
    Picture,
    /// Delegate comparison to an external command.
    External { command: String },
}

/// Parser configuration for tabular file formats.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableParser {
    /// Field delimiter character (e.g., ',', '\t').
    pub delimiter: char,
    /// Quote character for fields (e.g., '"', '\''). `None` means no quoting.
    pub quote: Option<char>,
    /// Escape character inside quoted fields. `None` means doubling the
    /// quote.
    pub escape: Option<char>,
    /// Number of header rows at the top of the file.
    pub header_rows: usize,
}

/// Encoding detection and normalization settings for text formats.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextEncoding {
    /// UTF-8 (default).
    #[default]
    Utf8,
    /// UTF-16 with BOM.
    Utf16,
    /// ISO-8859-1 (Latin-1).
    Latin1,
    /// Auto-detect from BOM or byte patterns.
    Auto,
}

/// Line ending convention for text files.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineEnding {
    /// Unix-style LF (`\n`).
    #[default]
    Lf,
    /// Windows-style CRLF (`\r\n`).
    Crlf,
    /// Classic Mac-style CR (`\r`).
    Cr,
    /// Treat all line endings as equivalent during comparison.
    Ignore,
}

/// Format-specific settings that control encoding, line endings, and other
/// conversion details.
#[derive(Clone, Debug, Default)]
pub struct FormatSettings {
    /// Text encoding for reading and writing.
    pub encoding: TextEncoding,
    /// Expected line ending convention.
    pub line_ending: LineEnding,
    /// Whether the file has a BOM (byte-order mark).
    pub has_bom: bool,
}

/// A file format definition that links extensions and MIME types to a
/// comparison mode, grammar, and default settings.
#[derive(Clone, Debug)]
pub struct FileFormat {
    /// Unique identifier (e.g., "rust", "csv", "png").
    pub id: String,
    /// Human-readable name (e.g., "Rust Source", "CSV").
    pub name: String,
    /// Glob patterns for file extensions (e.g., `"*.rs"`, `"*.toml"`).
    pub extensions: Vec<String>,
    /// MIME types associated with this format.
    pub mime_types: Vec<String>,
    /// Comparison mode (text, table, hex, picture, external).
    pub format_type: FormatType,
    /// Optional grammar for syntax-aware text comparison.
    pub grammar: Option<Grammar>,
    /// Format-specific settings (encoding, line endings, etc.).
    pub settings: FormatSettings,
}

/// Registry of known file formats, indexed by ID.
///
/// Provides lookup by file extension, MIME type, or format ID. Built-in
/// formats are registered at construction time; additional formats can be
/// added dynamically.
#[derive(Clone, Debug, Default)]
pub struct FormatRegistry {
    formats: HashMap<String, FileFormat>,
}

impl FormatRegistry {
    /// Create a new registry with built-in formats.
    pub fn new() -> Self {
        let mut reg = Self {
            formats: HashMap::new(),
        };
        reg.register_defaults();
        reg
    }

    /// Register a file format in the registry.
    pub fn register(&mut self, format: FileFormat) {
        self.formats.insert(format.id.clone(), format);
    }

    /// Look up a format by its ID.
    pub fn get(&self, id: &str) -> Option<&FileFormat> {
        self.formats.get(id)
    }

    /// Look up a format by file extension (e.g., `"foo.rs"` -> Rust format).
    pub fn by_extension(&self, path: &str) -> Option<&FileFormat> {
        path.rsplit('.').next().and_then(|ext| {
            let pattern = format!(".{}", ext.to_lowercase());
            self.formats.values().find(|f| {
                f.extensions.iter().any(|e| {
                    let e_lower = e.to_lowercase();
                    e_lower == pattern || e_lower == format!("*{}", pattern)
                })
            })
        })
    }

    /// Look up a format by MIME type.
    pub fn by_mime_type(&self, mime: &str) -> Option<&FileFormat> {
        self.formats.values().find(|f| {
            f.mime_types.iter().any(|m| m.eq_ignore_ascii_case(mime))
        })
    }

    /// Return all registered formats.
    pub fn all(&self) -> impl Iterator<Item = &FileFormat> {
        self.formats.values()
    }
}

impl FormatRegistry {
    /// Register built-in formats for common file types.
    fn register_defaults(&mut self) {
        // Rust source files.
        self.register(FileFormat {
            id: "rust".into(),
            name: "Rust Source".into(),
            extensions: vec!["*.rs".into()],
            mime_types: vec!["text/x-rust".into()],
            format_type: FormatType::Text,
            grammar: Some(Grammar::rust()),
            settings: FormatSettings::default(),
        });

        // Python source files.
        self.register(FileFormat {
            id: "python".into(),
            name: "Python Source".into(),
            extensions: vec!["*.py".into()],
            mime_types: vec!["text/x-python".into()],
            format_type: FormatType::Text,
            grammar: Some(Grammar::python()),
            settings: FormatSettings::default(),
        });

        // C/C++ source files.
        self.register(FileFormat {
            id: "c".into(),
            name: "C/C++ Source".into(),
            extensions: vec!["*.c".into(), "*.cpp".into(), "*.h".into()],
            mime_types: vec![
                "text/x-c".into(),
                "text/x-c++".into(),
                "text/x-chdr".into(),
            ],
            format_type: FormatType::Text,
            grammar: Some(Grammar::c()),
            settings: FormatSettings::default(),
        });

        // Generic plain text.
        self.register(FileFormat {
            id: "text".into(),
            name: "Plain Text".into(),
            extensions: vec!["*.txt".into(), "*.md".into()],
            mime_types: vec!["text/plain".into()],
            format_type: FormatType::Text,
            grammar: Some(Grammar::plain_text()),
            settings: FormatSettings::default(),
        });

        // CSV tabular data.
        self.register(FileFormat {
            id: "csv".into(),
            name: "CSV".into(),
            extensions: vec!["*.csv".into()],
            mime_types: vec!["text/csv".into()],
            format_type: FormatType::Table {
                parser: TableParser {
                    delimiter: ',',
                    quote: Some('"'),
                    escape: None,
                    header_rows: 1,
                },
            },
            grammar: None,
            settings: FormatSettings::default(),
        });

        // TSV tabular data.
        self.register(FileFormat {
            id: "tsv".into(),
            name: "TSV".into(),
            extensions: vec!["*.tsv".into()],
            mime_types: vec!["text/tab-separated-values".into()],
            format_type: FormatType::Table {
                parser: TableParser {
                    delimiter: '\t',
                    quote: None,
                    escape: None,
                    header_rows: 1,
                },
            },
            grammar: None,
            settings: FormatSettings::default(),
        });

        // PNG images.
        self.register(FileFormat {
            id: "png".into(),
            name: "PNG Image".into(),
            extensions: vec!["*.png".into()],
            mime_types: vec!["image/png".into()],
            format_type: FormatType::Picture,
            grammar: None,
            settings: FormatSettings::default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::Importance;

    #[test]
    fn registry_has_builtins() {
        let reg = FormatRegistry::new();
        assert!(reg.get("rust").is_some());
        assert!(reg.get("python").is_some());
        assert!(reg.get("csv").is_some());
        assert!(reg.get("text").is_some());
    }

    #[test]
    fn lookup_by_extension() {
        let reg = FormatRegistry::new();
        let fmt = reg.by_extension("foo.rs");
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().id, "rust");
    }

    #[test]
    fn lookup_by_mime_type() {
        let reg = FormatRegistry::new();
        let fmt = reg.by_mime_type("text/csv");
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().id, "csv");
    }

    #[test]
    fn lookup_unknown_extension_returns_none() {
        let reg = FormatRegistry::new();
        assert!(reg.by_extension("foo.xyz").is_none());
    }

    #[test]
    fn rust_format_has_grammar() {
        let reg = FormatRegistry::new();
        let fmt = reg.get("rust").unwrap();
        assert!(fmt.grammar.is_some());
        let g = fmt.grammar.as_ref().unwrap();
        assert_eq!(g.classify_line("// comment"), Importance::Comment);
    }

    #[test]
    fn csv_format_is_table() {
        let reg = FormatRegistry::new();
        let fmt = reg.get("csv").unwrap();
        assert!(matches!(fmt.format_type, FormatType::Table { .. }));
    }
}
