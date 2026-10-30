// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application options: the personalization layer of spec section 7.
//!
//! [`AppOptions`] is a gpui global holding the user's preferences — startup
//! behavior, global row-color overrides, and per-command visibility and
//! key bindings. It persists as JSON next to the workspace file, and the
//! Tools → Options dialog edits a working copy that is only applied when
//! the user confirms.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context as _, Result};
use cocomo_lib::ColorOverrides;
use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, Rgba,
    SharedString, Window,
    base::Selectable as _,
    component::{
        ActiveTheme, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputState},
        label::Label,
        switch::Switch,
        v_flex,
    },
    div,
    prelude::*,
};
use serde::{Deserialize, Serialize};

use crate::{commands, log, menus};

// ---------------------------------------------------------------------------
// Options model
// ---------------------------------------------------------------------------

/// How one command appears to the user.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandSetting {
    /// Whether the command's menu item and toolbar button are shown.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// The user's key binding, when it replaces the default.
    #[serde(default)]
    pub binding: Option<String>,
}

impl Default for CommandSetting {
    fn default() -> Self {
        Self {
            visible: true,
            binding: None,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The persisted application preferences.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppOptions {
    /// Reopen the last workspace's windows and tabs at launch.
    #[serde(default = "default_true")]
    pub restore_workspace_on_launch: bool,
    /// Snapshot dirty sessions periodically so a crash loses little work.
    #[serde(default = "default_true")]
    pub auto_save_sessions: bool,
    /// Global row-color overrides (spec section 7.1).
    #[serde(default)]
    pub colors: ColorOverrides,
    /// Per-command personalization, keyed by command id.
    #[serde(default)]
    pub commands: HashMap<String, CommandSetting>,
}

impl Default for AppOptions {
    fn default() -> Self {
        // A derived `Default` would make the bools `false`, silently
        // disabling restore and auto-save on first launch.
        Self {
            restore_workspace_on_launch: true,
            auto_save_sessions: true,
            colors: ColorOverrides::default(),
            commands: HashMap::new(),
        }
    }
}

impl AppOptions {
    /// Whether `id`'s menu item and toolbar button should be shown.
    pub fn command_visible(&self, id: &str) -> bool {
        self.commands.get(id).is_none_or(|c| c.visible)
    }

    /// The effective binding for `id`: the user's override when set.
    ///
    /// `Some("")` means the user explicitly unbound the command.
    pub fn command_binding(&self, id: &str) -> Option<&str> {
        self.commands.get(id)?.binding.as_deref()
    }
}

impl gpui::Global for AppOptions {}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Path of the options file.
pub fn options_path() -> PathBuf {
    crate::workspace::workspace_path()
        .parent()
        .map(|dir| dir.join("options.json"))
        .unwrap_or_else(|| PathBuf::from("cocomo-options.json"))
}

/// Load the persisted options, falling back to the defaults.
///
/// A corrupt file is treated as absent: preferences must never prevent
/// the application from starting.
pub fn load_options() -> AppOptions {
    fs_err::read_to_string(options_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Persist `options`, creating the config directory as needed.
pub fn save_options(options: &AppOptions) -> Result<()> {
    let path = options_path();
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent).with_context(|| {
            format!("failed to create config directory: {:?}", parent)
        })?;
    }
    let text = serde_json::to_string_pretty(options)
        .context("failed to serialize the options")?;
    fs_err::write(&path, text)
        .with_context(|| format!("failed to write options: {:?}", path))?;
    Ok(())
}

/// Install the options global, loading it from disk.
pub fn init_global(cx: &mut App) {
    cx.set_global(load_options());
}

/// Whether the command is shown, reading the options global.
pub fn command_visible(id: &str, cx: &App) -> bool {
    cx.global::<AppOptions>().command_visible(id)
}

// ---------------------------------------------------------------------------
// Options dialog
// ---------------------------------------------------------------------------

/// The dialog's page tabs, in order.
const PAGES: [&str; 3] = ["General", "Colors", "Commands"];

/// The color fields of the Colors page, in display order.
const COLOR_FIELDS: [(&str, &str); 4] = [
    ("important", "Important difference"),
    ("minor", "Minor difference"),
    ("orphan", "Orphan entry"),
    ("other_side", "Older side"),
];

/// Working state of an open Options dialog.
///
/// The dialog builder re-runs on every layer rebuild, so the edited values
/// and their input entities live here rather than in the builder closure.
struct OptionsDraft {
    /// Index into [`PAGES`].
    page: usize,
    /// The copy being edited; only applied on OK.
    working: AppOptions,
    /// Color inputs, created on first visit of the Colors page.
    color_inputs: Option<HashMap<String, Entity<InputState>>>,
    /// Binding inputs, created on first visit of the Commands page.
    binding_inputs: Option<HashMap<String, Entity<InputState>>>,
    /// Validation problem to show in the footer, if any.
    message: Option<SharedString>,
}

/// Open the Tools → Options dialog in the given window.
pub fn open_options_dialog(window: &mut Window, cx: &mut App) {
    let working = cx.global::<AppOptions>().clone();
    let draft = cx.new(|_| OptionsDraft {
        page: 0,
        working,
        color_inputs: None,
        binding_inputs: None,
        message: None,
    });
    window.open_dialog(cx, move |dialog, window, cx| {
        let (page, message) = {
            let d = draft.read(cx);
            (d.page, d.message.clone())
        };

        let tabs = h_flex().gap_1().children(PAGES.iter().enumerate().map(
            |(i, title)| {
                let draft_tab = draft.clone();
                let title: SharedString = (*title).into();
                Button::new(("options_page", i))
                    .label(title.clone())
                    .ghost()
                    .small()
                    .selected(i == page)
                    .accessibility_label(title)
                    .on_click(move |_, _, cx| {
                        draft_tab.update(cx, |d, cx| {
                            d.page = i;
                            d.message = None;
                            cx.notify();
                        });
                    })
            },
        ));

        let body = match page {
            1 => render_colors_page(&draft, window, cx),
            2 => render_commands_page(&draft, window, cx),
            _ => render_general_page(&draft, cx),
        };

        let footer_message = message.map(|text| {
            div().text_xs().text_color(cx.theme().danger).child(text)
        });

        let draft_ok = draft.clone();
        dialog
            .title("Options")
            .child(v_flex().gap_3().child(tabs).child(body))
            .footer(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .children(footer_message)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("options_cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                    }),
                            )
                            .child(
                                Button::new("options_ok")
                                    .label("OK")
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        apply_options(draft_ok.clone(), cx);
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ),
            )
    });
}

/// Page 1: startup and persistence behavior.
fn render_general_page(draft: &Entity<OptionsDraft>, cx: &App) -> AnyElement {
    let restore = draft.read(cx).working.restore_workspace_on_launch;
    let auto_save = draft.read(cx).working.auto_save_sessions;

    let restore_switch = {
        let draft_restore = draft.clone();
        Switch::new("options_restore_workspace")
            .label("Restore the last workspace at launch")
            .checked(restore)
            .on_click(move |checked, _, cx| {
                let checked = *checked;
                draft_restore.update(cx, |d, cx| {
                    d.working.restore_workspace_on_launch = checked;
                    cx.notify();
                });
            })
    };
    let auto_save_switch = {
        let draft_save = draft.clone();
        Switch::new("options_auto_save")
            .label("Auto-save unsaved sessions")
            .checked(auto_save)
            .on_click(move |checked, _, cx| {
                let checked = *checked;
                draft_save.update(cx, |d, cx| {
                    d.working.auto_save_sessions = checked;
                    cx.notify();
                });
            })
    };

    v_flex()
        .gap_3()
        .child(restore_switch)
        .child(auto_save_switch)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Auto-save writes a snapshot of open sessions so an \
                     unexpected quit can be recovered at the next launch.",
                ),
        )
        .into_any_element()
}

/// Page 2: the global row-color overrides.
fn render_colors_page(
    draft: &Entity<OptionsDraft>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let inputs = ensure_color_inputs(draft, window, cx);
    let theme = cx.theme();

    v_flex()
        .gap_2()
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            "Hex colors (#rrggbb) that override the theme for every view. A \
             session's own colors take precedence. Leave a field empty to \
             keep the theme color.",
        ))
        .children(COLOR_FIELDS.iter().map(|(key, label)| {
            let input = inputs
                .get(*key)
                .cloned()
                .expect("color inputs created above");
            h_flex()
                .items_center()
                .gap_2()
                .child(div().w_40().flex_shrink_0().child(Label::new(*label)))
                .child(
                    div().flex_1().min_w_0().child(Input::new(&input).small()),
                )
        }))
        .into_any_element()
}

/// Page 3: command visibility and key bindings.
fn render_commands_page(
    draft: &Entity<OptionsDraft>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let inputs = ensure_binding_inputs(draft, window, cx);
    let theme = cx.theme();
    let specs = commands::all_commands();

    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    let mut category: Option<&str> = None;
    for spec in specs {
        if category != Some(spec.category) {
            category = Some(spec.category);
            rows.push(
                div()
                    .pt_2()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(spec.category))
                    .into_any_element(),
            );
        }
        let visible = draft.read(cx).working.command_visible(spec.id);
        let draft_row = draft.clone();
        let id = spec.id;
        let checkbox = Checkbox::new(ElementId::Name(
            format!("options_visible_{}", spec.id).into(),
        ))
        .checked(visible)
        .accessibility_label(format!("Show {}", spec.label))
        .on_click(move |checked, _, cx| {
            let checked = *checked;
            draft_row.update(cx, |d, cx| {
                d.working
                    .commands
                    .entry(id.to_string())
                    .or_default()
                    .visible = checked;
                cx.notify();
            });
        });

        let binding_cell = if let Some(input) = inputs.get(spec.id) {
            div()
                .w_32()
                .flex_shrink_0()
                .child(Input::new(input).small())
        } else {
            div().w_32().flex_shrink_0()
        };

        rows.push(
            h_flex()
                .items_center()
                .gap_2()
                .child(checkbox)
                .child(div().flex_1().min_w_0().child(Label::new(spec.label)))
                .child(binding_cell)
                .into_any_element(),
        );
    }

    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            "Hide commands you never use, or rebind them. Press Enter in a \
             binding field to apply it; an invalid sequence is reported when \
             you click OK.",
        ))
        .child(
            div()
                .id("options-commands-scroll")
                .h_80()
                .overflow_y_scroll()
                .pr_2()
                .child(v_flex().gap_0p5().children(rows)),
        )
        .into_any_element()
}

/// Create the color inputs once, seeded from the working options.
fn ensure_color_inputs(
    draft: &Entity<OptionsDraft>,
    window: &mut Window,
    cx: &mut App,
) -> HashMap<String, Entity<InputState>> {
    let existing = draft.read(cx).color_inputs.clone();
    if let Some(inputs) = existing {
        return inputs;
    }
    let seeded = color_values(&draft.read(cx).working.colors);
    let inputs: HashMap<String, Entity<InputState>> = seeded
        .into_iter()
        .map(|(key, value)| {
            let input = cx.new(|cx| {
                let mut state =
                    InputState::new(window, cx).placeholder("#rrggbb");
                if !value.is_empty() {
                    state.set_value(value, window, cx);
                }
                state
            });
            (key, input)
        })
        .collect();
    draft.update(cx, |d, _| d.color_inputs = Some(inputs.clone()));
    inputs
}

/// Create the binding inputs once, seeded from the effective bindings.
fn ensure_binding_inputs(
    draft: &Entity<OptionsDraft>,
    window: &mut Window,
    cx: &mut App,
) -> HashMap<String, Entity<InputState>> {
    let existing = draft.read(cx).binding_inputs.clone();
    if let Some(inputs) = existing {
        return inputs;
    }
    let seeds: Vec<(String, String)> = commands::all_commands()
        .into_iter()
        .filter(|spec| spec.is_bindable())
        .map(|spec| {
            let value = draft
                .read(cx)
                .working
                .command_binding(spec.id)
                .or(spec.default_binding)
                .unwrap_or_default()
                .to_string();
            (spec.id.to_string(), value)
        })
        .collect();
    let inputs: HashMap<String, Entity<InputState>> = seeds
        .into_iter()
        .map(|(id, value)| {
            let input = cx.new(|cx| {
                let mut state =
                    InputState::new(window, cx).placeholder("Unbound");
                if !value.is_empty() {
                    state.set_value(value, window, cx);
                }
                state
            });
            (id, input)
        })
        .collect();
    draft.update(cx, |d, _| d.binding_inputs = Some(inputs.clone()));
    inputs
}

/// Validate the draft, then persist and apply it.
///
/// An invalid color or binding aborts the apply and leaves the dialog
/// open with a message, so a typo never silently drops a preference.
fn apply_options(draft: Entity<OptionsDraft>, cx: &mut App) {
    let (working, color_inputs, binding_inputs) = {
        let d = draft.read(cx);
        (
            d.working.clone(),
            d.color_inputs.clone(),
            d.binding_inputs.clone(),
        )
    };

    let mut colors = ColorOverrides::default();
    if let Some(inputs) = &color_inputs {
        for (key, input) in inputs {
            let value = input.read(cx).value().trim().to_string();
            if value.is_empty() {
                continue;
            }
            if let Err(e) = Rgba::try_from(value.as_str()) {
                set_message(
                    &draft,
                    format!("\"{value}\" is not a valid color. {e}"),
                    cx,
                );
                return;
            }
            match key.as_str() {
                "important" => colors.important = Some(value),
                "minor" => colors.minor = Some(value),
                "orphan" => colors.orphan = Some(value),
                "other_side" => colors.other_side = Some(value),
                _ => {}
            }
        }
    }

    let mut options = working;
    options.colors = colors;
    if let Some(inputs) = &binding_inputs {
        for spec in commands::all_commands() {
            let Some(input) = inputs.get(spec.id) else {
                continue;
            };
            let value = input.read(cx).value().trim().to_string();
            if value.is_empty() {
                // An empty field means "unbound". The sentinel empty
                // override tells the keymap rebuild to drop the default
                // binding without adding a replacement.
                options
                    .commands
                    .entry(spec.id.to_string())
                    .or_default()
                    .binding = Some(String::new());
                continue;
            }
            if let Err(message) = commands::parse_binding(&spec, &value) {
                set_message(&draft, message, cx);
                return;
            }
            options
                .commands
                .entry(spec.id.to_string())
                .or_default()
                .binding = Some(value);
        }
    }

    if let Err(e) = save_options(&options) {
        set_message(&draft, format!("Couldn't save the options. {e}"), cx);
        return;
    }
    cx.set_global(options);
    commands::rebuild_keymap(cx);
    menus::rebuild_menus(cx);
    crate::windows::refresh_all_windows(cx);
    log::push(log::LogLevel::Info, "Options updated".to_string(), cx);
}

/// Show a validation problem inside the open dialog.
fn set_message(
    draft: &Entity<OptionsDraft>,
    message: impl Into<SharedString>,
    cx: &mut App,
) {
    let message = message.into();
    draft.update(cx, |d, cx| {
        d.message = Some(message);
        cx.notify();
    });
}

/// The override values keyed by the [`COLOR_FIELDS`] keys.
fn color_values(colors: &ColorOverrides) -> HashMap<String, String> {
    [
        ("important", &colors.important),
        ("minor", &colors.minor),
        ("orphan", &colors.orphan),
        ("other_side", &colors.other_side),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.clone().unwrap_or_default()))
    .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_restore_and_auto_save() {
        let options = AppOptions::default();
        assert!(options.restore_workspace_on_launch);
        assert!(options.auto_save_sessions);
        assert!(options.command_visible("session.save"));
        assert_eq!(options.command_binding("session.save"), None);
    }

    #[test]
    fn hidden_command_reports_invisible() {
        let mut options = AppOptions::default();
        options.commands.insert(
            "toolbar.home".to_string(),
            CommandSetting {
                visible: false,
                binding: None,
            },
        );
        assert!(!options.command_visible("toolbar.home"));
        assert!(options.command_visible("toolbar.rules"));
    }

    #[test]
    fn options_round_trip_through_json() {
        let mut options = AppOptions::default();
        options.colors.important = Some("#ff0000".to_string());
        options.commands.insert(
            "session.save".to_string(),
            CommandSetting {
                visible: true,
                binding: Some("cmd-s".to_string()),
            },
        );
        let text = serde_json::to_string(&options).unwrap();
        let back: AppOptions = serde_json::from_str(&text).unwrap();
        assert_eq!(back.colors.important.as_deref(), Some("#ff0000"));
        assert_eq!(
            back.command_binding("session.save"),
            Some("cmd-s".to_string().as_str())
        );
    }

    #[test]
    fn partial_json_keeps_missing_fields_at_their_defaults() {
        let back: AppOptions =
            serde_json::from_str(r#"{"colors": {}}"#).unwrap();
        assert!(back.restore_workspace_on_launch);
        assert!(back.auto_save_sessions);
    }
}
