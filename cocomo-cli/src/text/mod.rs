// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

mod diff;

use std::path::Path;

use cocomo_lib::{
    FileSystem,
    text::{TextCompareSettings, compare_texts},
};
use diff::{generate_unified_diff, print_text_diff_table};

use crate::{
    cli::{
        GrammarArg, TextCommand, TextCompareArgs, TextDiffArgs, WhitespaceArg,
    },
    error::{CliError, DiffResult},
    resolver::{EndpointResolver, resolve_endpoint},
};

// ---------------------------------------------------------------------------
// Text commands
// ---------------------------------------------------------------------------

pub(crate) async fn run_text<R: EndpointResolver>(
    cmd: &TextCommand,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match cmd {
        TextCommand::Compare(args) => text_compare(args, resolver).await,
        TextCommand::Diff(args) => text_diff(args, resolver).await,
    }
}

pub(super) fn build_text_settings(
    ignore_case: bool,
    whitespace: WhitespaceArg,
    ignore_blank_lines: bool,
    ignore_comments: bool,
    grammar: Option<GrammarArg>,
) -> TextCompareSettings {
    let mut settings = TextCompareSettings::new();
    settings.ignore_case = ignore_case;
    settings.whitespace_mode = whitespace.clone().into();
    settings.ignore_blank_lines = ignore_blank_lines;
    settings.ignore_comments = ignore_comments;
    if let Some(g) = grammar {
        settings.grammar = Some(g.into());
    }
    settings
}

pub(super) async fn text_compare<R: EndpointResolver>(
    args: &TextCompareArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (left_fs, left_path) =
        resolve_endpoint(resolver, &args.left, args.profile.as_deref())?;
    let (right_fs, right_path) =
        resolve_endpoint(resolver, &args.right, args.profile.as_deref())?;
    let left_bytes = left_fs.read(&left_path, None).await?;
    let right_bytes = right_fs.read(&right_path, None).await?;
    let left_content = String::from_utf8_lossy(&left_bytes);
    let right_content = String::from_utf8_lossy(&right_bytes);

    let settings = build_text_settings(
        args.ignore_case,
        args.ignore_whitespace.clone(),
        args.ignore_blank_lines,
        args.ignore_comments,
        args.grammar.clone(),
    );

    let diff = compare_texts(&left_content, &right_content, &settings);

    if diff.is_empty() {
        println!("Files are identical.");
        return Ok(DiffResult::NoDiffs);
    }

    let left_name = left_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| args.left.clone());
    let right_name = right_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| args.right.clone());

    print_text_diff_table(&diff, &left_name, &right_name);

    println!(
        "\n{} differences found ({} lines left, {} lines right)",
        diff.differences.len(),
        diff.left_line_count,
        diff.right_line_count,
    );

    Ok(DiffResult::HasDiffs)
}

pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // The cut point may fall inside a multi-byte character, so snap down to
    // the nearest char boundary before slicing.
    let end = s.floor_char_boundary(max.saturating_sub(3));
    format!("{}...", &s[..end])
}

pub(super) async fn text_diff<R: EndpointResolver>(
    args: &TextDiffArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (left_fs, left_path) =
        resolve_endpoint(resolver, &args.left, args.profile.as_deref())?;
    let (right_fs, right_path) =
        resolve_endpoint(resolver, &args.right, args.profile.as_deref())?;
    let left_bytes = left_fs.read(&left_path, None).await?;
    let right_bytes = right_fs.read(&right_path, None).await?;
    let left_content = String::from_utf8_lossy(&left_bytes);
    let right_content = String::from_utf8_lossy(&right_bytes);

    let settings = build_text_settings(
        args.ignore_case,
        args.ignore_whitespace.clone(),
        args.ignore_blank_lines,
        args.ignore_comments,
        args.grammar.clone(),
    );

    let diff = compare_texts(&left_content, &right_content, &settings);

    if diff.is_empty() {
        return Ok(DiffResult::NoDiffs);
    }

    // Generate unified diff from the TextDiff result.
    let unified = generate_unified_diff(
        Path::new(&args.left),
        Path::new(&args.right),
        &left_content,
        &right_content,
        &diff,
    );

    print!("{}", unified);

    Ok(DiffResult::HasDiffs)
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn short_string_is_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn long_string_is_truncated_with_ellipsis() {
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
    }

    #[test]
    fn cut_point_inside_multibyte_char_does_not_panic() {
        // '✅' occupies bytes 56..59, so a naive cut at byte 57 would panic.
        let line = format!("{}✅tail", "x".repeat(56));
        assert_eq!(truncate(&line, 60), format!("{}...", "x".repeat(56)));
    }

    #[test]
    fn max_smaller_than_ellipsis_returns_only_ellipsis() {
        assert_eq!(truncate("abcdef", 2), "...");
    }
}
