// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # File Comparison Module (`filediff`)
//!
//! This module provides the logic for comparing two text files line by line.
//! It computes the differences and prepares them for side-by-side display.

use std::{fs, io};

use similar::{ChangeTag, TextDiff};

use crate::FSItem;

/// The type of change for a single line in a file comparison.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LineDiffType {
    /// Line exists only on the left side.
    Removed,
    /// Line exists only on the right side.
    Added,
    /// Line is the same on both sides.
    Unchanged,
    /// Line is partially different on both sides.
    Changed,
}

/// A single line in a file comparison result, potentially representing a
/// placeholder if the line only exists on the other side.
#[derive(Clone, Debug)]
pub struct DiffLine {
    /// The line number (1-based).
    pub line_number: Option<usize>,
    /// The content of the line.
    pub content: String,
}

/// A chunk of adjacent lines with the same diff type.
#[derive(Clone, Debug)]
pub struct DiffChunk {
    /// The type of difference for this chunk.
    pub diff_type: LineDiffType,
    /// The lines on the left side.
    pub left_lines: Vec<DiffLine>,
    /// The lines on the right side.
    pub right_lines: Vec<DiffLine>,
}

/// A complete result of a comparison between two text files.
#[derive(Clone, Debug)]
pub struct FileDiff {
    /// The source file on the left side.
    pub left_file: FSItem,
    /// The source file on the right side.
    pub right_file: FSItem,
    /// The list of compared chunks.
    pub chunks: Vec<DiffChunk>,
}

impl FileDiff {
    /// Compares the contents of two text files.
    pub async fn new(
        left_file: Option<FSItem>,
        right_file: Option<FSItem>,
    ) -> io::Result<Self> {
        let left_content = if let Some(ref f) = left_file {
            fs::read_to_string(f.path())?
        } else {
            String::new()
        };
        let right_content = if let Some(ref f) = right_file {
            fs::read_to_string(f.path())?
        } else {
            String::new()
        };

        let diff = TextDiff::from_lines(&left_content, &right_content);
        let mut chunks = Vec::new();

        for op in diff.ops() {
            match op {
                similar::DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } => {
                    let mut left_lines = Vec::new();
                    let mut right_lines = Vec::new();
                    for i in 0..*len {
                        let line = diff.iter_changes(op).nth(i).unwrap();
                        left_lines.push(DiffLine {
                            line_number: Some(old_index + i + 1),
                            content: line.value().to_string(),
                        });
                        right_lines.push(DiffLine {
                            line_number: Some(new_index + i + 1),
                            content: line.value().to_string(),
                        });
                    }
                    chunks.push(DiffChunk {
                        diff_type: LineDiffType::Unchanged,
                        left_lines,
                        right_lines,
                    });
                }
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => {
                    let mut left_lines = Vec::new();
                    let mut right_lines = Vec::new();
                    for i in 0..*old_len {
                        let line = diff.iter_changes(op).nth(i).unwrap();
                        left_lines.push(DiffLine {
                            line_number: Some(old_index + i + 1),
                            content: line.value().to_string(),
                        });
                        right_lines.push(DiffLine {
                            line_number: None,
                            content: String::new(),
                        });
                    }
                    chunks.push(DiffChunk {
                        diff_type: LineDiffType::Removed,
                        left_lines,
                        right_lines,
                    });
                }
                similar::DiffOp::Insert {
                    new_index, new_len, ..
                } => {
                    let mut left_lines = Vec::new();
                    let mut right_lines = Vec::new();
                    for i in 0..*new_len {
                        let line = diff.iter_changes(op).nth(i).unwrap();
                        left_lines.push(DiffLine {
                            line_number: None,
                            content: String::new(),
                        });
                        right_lines.push(DiffLine {
                            line_number: Some(new_index + i + 1),
                            content: line.value().to_string(),
                        });
                    }
                    chunks.push(DiffChunk {
                        diff_type: LineDiffType::Added,
                        left_lines,
                        right_lines,
                    });
                }
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    let mut left_lines = Vec::new();
                    let mut right_lines = Vec::new();
                    let common_len = (*old_len).min(*new_len);

                    // Map overlapping lines to 'Changed'
                    for i in 0..common_len {
                        let left_line = diff
                            .iter_changes(op)
                            .filter(|c| c.tag() == ChangeTag::Delete)
                            .nth(i)
                            .unwrap();
                        let right_line = diff
                            .iter_changes(op)
                            .filter(|c| c.tag() == ChangeTag::Insert)
                            .nth(i)
                            .unwrap();
                        left_lines.push(DiffLine {
                            line_number: Some(old_index + i + 1),
                            content: left_line.value().to_string(),
                        });
                        right_lines.push(DiffLine {
                            line_number: Some(new_index + i + 1),
                            content: right_line.value().to_string(),
                        });
                    }
                    chunks.push(DiffChunk {
                        diff_type: LineDiffType::Changed,
                        left_lines,
                        right_lines,
                    });

                    // Handle remaining lines in the Replace op
                    if old_len > new_len {
                        let mut rem_left = Vec::new();
                        let mut rem_right = Vec::new();
                        for i in common_len..*old_len {
                            let left_line = diff
                                .iter_changes(op)
                                .filter(|c| c.tag() == ChangeTag::Delete)
                                .nth(i)
                                .unwrap();
                            rem_left.push(DiffLine {
                                line_number: Some(old_index + i + 1),
                                content: left_line.value().to_string(),
                            });
                            rem_right.push(DiffLine {
                                line_number: None,
                                content: String::new(),
                            });
                        }
                        chunks.push(DiffChunk {
                            diff_type: LineDiffType::Removed,
                            left_lines: rem_left,
                            right_lines: rem_right,
                        });
                    } else if new_len > old_len {
                        let mut rem_left = Vec::new();
                        let mut rem_right = Vec::new();
                        for i in common_len..*new_len {
                            let right_line = diff
                                .iter_changes(op)
                                .filter(|c| c.tag() == ChangeTag::Insert)
                                .nth(i)
                                .unwrap();
                            rem_left.push(DiffLine {
                                line_number: None,
                                content: String::new(),
                            });
                            rem_right.push(DiffLine {
                                line_number: Some(new_index + i + 1),
                                content: right_line.value().to_string(),
                            });
                        }
                        chunks.push(DiffChunk {
                            diff_type: LineDiffType::Added,
                            left_lines: rem_left,
                            right_lines: rem_right,
                        });
                    }
                }
            }
        }

        Ok(Self {
            left_file: left_file.unwrap_or_default(),
            right_file: right_file.unwrap_or_default(),
            chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    async fn create_test_file(content: &str) -> (NamedTempFile, FSItem) {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        let path = file.path().to_path_buf();
        let fs_item = FSItem::new(path).await;
        (file, fs_item)
    }

    #[tokio::test]
    async fn test_file_diff() {
        let left_content = "line1\nline2\nline3\n";
        let right_content = "line1\nline2.modified\nline4\n";

        let (_l_file, _l_item) = create_test_file(left_content).await;
        let (_r_file, _r_item) = create_test_file(right_content).await;

        let diff = FileDiff::new(Some(_l_item), Some(_r_item)).await.unwrap();

        // 1. Equal { old_index: 0, new_index: 0, len: 1 }
        // 2. Replace { old_index: 1, old_len: 2, new_index: 1, new_len: 2 }
        assert_eq!(diff.chunks.len(), 2);

        // chunk 0 - unchanged
        assert_eq!(diff.chunks[0].diff_type, LineDiffType::Unchanged);
        assert_eq!(diff.chunks[0].left_lines[0].content, "line1\n");
        assert_eq!(diff.chunks[0].left_lines[0].line_number, Some(1));
        assert_eq!(diff.chunks[0].right_lines[0].content, "line1\n");
        assert_eq!(diff.chunks[0].right_lines[0].line_number, Some(1));

        // chunk 1 - changed (Replace op maps to Changed chunk)
        assert_eq!(diff.chunks[1].diff_type, LineDiffType::Changed);
        assert_eq!(diff.chunks[1].left_lines.len(), 2);

        assert_eq!(diff.chunks[1].left_lines[0].content, "line2\n");
        assert_eq!(diff.chunks[1].left_lines[0].line_number, Some(2));
        assert_eq!(diff.chunks[1].right_lines[0].content, "line2.modified\n");
        assert_eq!(diff.chunks[1].right_lines[0].line_number, Some(2));

        assert_eq!(diff.chunks[1].left_lines[1].content, "line3\n");
        assert_eq!(diff.chunks[1].left_lines[1].line_number, Some(3));
        assert_eq!(diff.chunks[1].right_lines[1].content, "line4\n");
        assert_eq!(diff.chunks[1].right_lines[1].line_number, Some(3));
    }

    #[tokio::test]
    async fn test_file_diff_add_remove() {
        let left_content = "line1\nline2\n";
        let right_content = "line1\nline3\nline4\n";

        let (_l_file, _l_item) = create_test_file(left_content).await;
        let (_r_file, _r_item) = create_test_file(right_content).await;

        let diff = FileDiff::new(Some(_l_item), Some(_r_item)).await.unwrap();

        // 1. Equal { old_index: 0, new_index: 0, len: 1 }
        // 2. Replace { old_index: 1, old_len: 1, new_index: 1, new_len: 2 }
        //    common_len = 1 -> Changed (1 line) new_len > old_len -> Added (1
        //    line)
        assert_eq!(diff.chunks.len(), 3);

        assert_eq!(diff.chunks[0].diff_type, LineDiffType::Unchanged);
        assert_eq!(diff.chunks[1].diff_type, LineDiffType::Changed);
        assert_eq!(diff.chunks[2].diff_type, LineDiffType::Added);

        assert_eq!(diff.chunks[1].left_lines[0].content, "line2\n");
        assert_eq!(diff.chunks[1].right_lines[0].content, "line3\n");

        assert_eq!(diff.chunks[2].left_lines[0].line_number, None);
        assert_eq!(diff.chunks[2].right_lines[0].content, "line4\n");
    }
}
