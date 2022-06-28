// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

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
// #![warn(clippy::mismatching_type_param_order)] TODO: enable when 1.62 stable
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
#![warn(clippy::string_to_string)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::unicode_not_nfc)]
#![warn(clippy::unimplemented)]
#![warn(clippy::unseparated_literal_suffix)]
#![warn(clippy::unused_self)]
#![warn(clippy::unwrap_in_result)]
#![warn(clippy::use_self)]
#![warn(clippy::used_underscore_binding)]
#![warn(clippy::wildcard_imports)]

mod app;
mod cmdargs;
mod cmdbar;
mod session;
mod tabbar;
mod terminal;
mod view;

use std::{io, rc::Rc};

use cmdargs::CmdLineArgs;
use cocomo_core::{FSItem, FSItemType};

use crate::{
    session::Session,
    terminal::{reset_terminal, setup_terminal, start_terminal},
};

fn exit_with_error(msg: String) {
    eprintln!("{}", msg);
    std::process::exit(1)
}

fn main() -> Result<(), io::Error> {
    let args = CmdLineArgs::get();
    if args.left.is_none() || args.right.is_none() {
        exit_with_error("Please specify left and right path!".to_string());
    }

    let left_item: FSItem;
    let right_item: FSItem;

    let mut left = args.left.unwrap();
    match FSItem::try_from(&left) {
        Ok(item) => left_item = item,
        Err(err) => {
            exit_with_error(format!("{}: {}", left, err.to_string()));
            unreachable!()
        }
    }
    let mut right = args.right.unwrap();
    match FSItem::try_from(&right) {
        Ok(item) => right_item = item,
        Err(err) => {
            exit_with_error(format!("{}: {}", right, err.to_string()));
            unreachable!()
        }
    }
    match (left_item.item_type(), right_item.item_type()) {
        (FSItemType::Directory, FSItemType::File { .. }) => {
            exit_with_error(format!(
                "Can't compare directory '{}' to file '{}'.",
                left, right
            ));
        }
        (FSItemType::File { .. }, FSItemType::Directory) => {
            exit_with_error(format!(
                "Can't compare file '{}' to directory '{}'.",
                left, right
            ));
        }
        (..) => {
            left = left_item.path().display().to_string();
            right = right_item.path().display().to_string();
        }
    }

    let session =
        Session::new(1, None, Rc::new(left_item), Rc::new(right_item));
    let mut app = app::App::new(session);
    setup_terminal()?;
    let mut terminal = start_terminal(io::stdout())?;
    app.run(&mut terminal)?;
    reset_terminal(&mut terminal)?;

    println!("Compare '{}' and '{}'!", left, right);

    Ok(())
}
