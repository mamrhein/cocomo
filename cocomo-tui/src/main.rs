// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Cocomo TUI
//!
//! This crate provides the Terminal User Interface for the Cocomo directory
//! comparison tool.

#![doc = include_str ! ("../../README.md")]
// activate some rustc lints
#![deny(non_ascii_idents)]
#![deny(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]
#![warn(trivial_casts, trivial_numeric_casts)]
#![warn(unused)]
#![allow(dead_code)]
// activate some clippy lints
#![warn(clippy::cast_possible_truncation)]
#![warn(clippy::cast_possible_wrap)]
#![warn(clippy::cast_precision_loss)]
#![warn(clippy::cast_sign_loss)]
#![warn(clippy::cognitive_complexity)]
#![warn(clippy::decimal_literal_representation)]
#![warn(clippy::enum_glob_use)]
#![warn(clippy::equatable_if_let)]
#![warn(clippy::fallible_impl_from)]
#![warn(clippy::if_not_else)]
#![warn(clippy::if_then_some_else_none)]
#![warn(clippy::implicit_clone)]
#![warn(clippy::integer_division)]
#![warn(clippy::manual_assert)]
#![warn(clippy::match_same_arms)]
#![warn(clippy::mismatching_type_param_order)]
#![warn(clippy::missing_const_for_fn)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::missing_panics_doc)]
#![warn(clippy::multiple_crate_versions)]
#![warn(clippy::multiple_inherent_impl)]
#![warn(clippy::must_use_candidate)]
#![warn(clippy::needless_pass_by_value)]
#![warn(clippy::print_stderr)]
#![warn(clippy::print_stdout)]
#![warn(clippy::semicolon_if_nothing_returned)]
#![warn(clippy::str_to_string)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::unicode_not_nfc)]
#![warn(clippy::unimplemented)]
#![warn(clippy::unseparated_literal_suffix)]
#![warn(clippy::unused_self)]
#![warn(clippy::unwrap_in_result)]
#![warn(clippy::use_self)]
#![warn(clippy::used_underscore_binding)]
#![warn(clippy::wildcard_imports)]

mod cmdargs;

/// Holds the state and application logic.
pub mod app;
/// Renders the directory comparison view.
pub mod dirview;
/// Handles the terminal events (key press, mouse click, resize, etc.).
pub mod event;
/// Renders the file comparison view.
pub mod fileview;
/// Renders the widgets / UI.
pub mod ui;

use cmdargs::CmdLineArgs;
use cocomo_core::{FSItem, FSItemType};
use color_eyre::Report;

use crate::app::App;

/// Validates the command line arguments and returns the left and right
/// [`FSItem`]s if they are valid for comparison.
async fn check_args(
    args: &CmdLineArgs,
) -> Result<(Option<FSItem>, Option<FSItem>), Report> {
    let left_item = if let Some(path) = &args.left {
        Some(FSItem::new(path).await)
    } else {
        None
    };
    let right_item = if let Some(path) = &args.right {
        Some(FSItem::new(path).await)
    } else {
        None
    };
    let left_item_type = if let Some(item) = left_item.as_ref() {
        Some(item.final_item_type().await.into_owned())
    } else {
        None
    };
    let right_item_type = if let Some(item) = right_item.as_ref() {
        Some(item.final_item_type().await.into_owned())
    } else {
        None
    };
    let err_report = match (&left_item_type, &right_item_type) {
        (
            Some(FSItemType::Invalid { cause: left_cause }),
            Some(FSItemType::Invalid { cause: right_cause }),
        ) => Some(Report::msg(format!(
            "{}: {}\n{}: {}",
            left_item.as_ref().unwrap().path().display(),
            left_cause,
            right_item.as_ref().unwrap().path().display(),
            right_cause
        ))),
        (Some(FSItemType::Invalid { cause }), ..) => {
            Some(Report::msg(format!(
                "{}: {}",
                left_item.as_ref().unwrap().path().display(),
                cause
            )))
        }
        (.., Some(FSItemType::Invalid { cause })) => {
            Some(Report::msg(format!(
                "{}: {}",
                right_item.as_ref().unwrap().path().display(),
                cause
            )))
        }
        (.., None)
        | (None, ..)
        | (Some(FSItemType::Directory), Some(FSItemType::Directory)) => None,
        (
            Some(FSItemType::File {
                file_type: left_file_type,
            }),
            Some(FSItemType::File {
                file_type: right_file_type,
            }),
        ) => (!left_item
            .as_ref()
            .unwrap()
            .comparable(right_item.as_ref().unwrap())
            .await)
            .then_some(Report::msg(format!(
                "Can't compare files of different type:\n'{}' <> '{}'.",
                left_file_type, right_file_type
            ))),
        _ => Some(Report::msg(format!(
            "Can't compare a {} and a {}.",
            // save to unwrap here
            left_item_type.unwrap(),
            right_item_type.unwrap()
        ))),
    };
    err_report.map_or_else(|| Ok((left_item, right_item)), Err)
}

#[tokio::main]
async fn main() -> Result<(), Report> {
    color_eyre::install()?;
    let args = CmdLineArgs::get();
    let (left, right) = check_args(&args).await?;
    let app = App::new(left, right).await;
    let terminal = ratatui::init();
    let result = app.run(terminal).await;
    ratatui::restore();
    result
}
