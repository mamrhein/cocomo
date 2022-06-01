// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

#[allow(dead_code)]
mod cmdargs;
mod terminal;

use std::io;

use cmdargs::CmdLineArgs;
use cocomo_core::{FSItem, ItemType};

use crate::terminal::{reset_terminal, setup_terminal, start_terminal};

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

    let left = args.left.unwrap();
    match FSItem::try_from(&left) {
        Ok(item) => left_item = item,
        Err(err) => {
            exit_with_error(format!("{}: {}", left, err.to_string()));
            unreachable!()
        }
    }
    let right = args.right.unwrap();
    match FSItem::try_from(&right) {
        Ok(item) => right_item = item,
        Err(err) => {
            exit_with_error(format!("{}: {}", right, err.to_string()));
            unreachable!()
        }
    }
    match (left_item.item_type, right_item.item_type) {
        (ItemType::Directory, ItemType::File) => {
            exit_with_error(format!(
                "Can't compare directory '{}' to file '{}'.",
                left, right
            ));
        }
        (ItemType::File, ItemType::Directory) => {
            exit_with_error(format!(
                "Can't compare file '{}' to directory '{}'.",
                left, right
            ));
        }
        (..) => {}
    }

    setup_terminal()?;
    let mut terminal = start_terminal(io::stdout())?;

    reset_terminal(&mut terminal)?;

    println!(
        "Compare '{}' and '{}'!",
        left_item.path.display(),
        right_item.path.display()
    );

    Ok(())
}
