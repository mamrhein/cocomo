// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Named folder-sync presets.
//!
//! A preset pairs a name with one whole-comparison [`SyncKind`], so a
//! recurring sync routine (say "Deploy" = mirror to right) is one click in
//! the sync view's toolbar instead of a submenu hunt. Presets persist in
//! [`crate::options::AppOptions`] alongside the other application
//! preferences; the Options menu dialog ([`open_sync_presets_dialog`])
//! manages the list.

use gpui::{
    App, ClickEvent, Context, Entity, IntoElement, Render, Window,
    assets::IconName,
    component::{
        ActiveTheme, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputState},
        label::Label,
        radio::Radio,
        v_flex,
    },
    div,
    prelude::*,
    px,
};
use serde::{Deserialize, Serialize};

use crate::{
    options::{AppOptions, save_options},
    state::SyncKind,
};

/// A named whole-comparison sync run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPreset {
    /// User-chosen name shown in the toolbar dropdown.
    pub name: String,
    /// The sync kind the preset runs.
    pub kind: SyncKind,
}

/// The action instance that runs `kind`, for dispatch from the sync
/// view's toolbar dropdown.
pub(crate) fn sync_preset_action(kind: SyncKind) -> Box<dyn gpui::Action> {
    use crate::ui::{
        SyncMirrorLeft, SyncMirrorRight, SyncUpdateBoth, SyncUpdateLeft,
        SyncUpdateRight,
    };
    match kind {
        SyncKind::UpdateRight => Box::new(SyncUpdateRight),
        SyncKind::UpdateLeft => Box::new(SyncUpdateLeft),
        SyncKind::UpdateBoth => Box::new(SyncUpdateBoth),
        SyncKind::MirrorRight => Box::new(SyncMirrorRight),
        SyncKind::MirrorLeft => Box::new(SyncMirrorLeft),
    }
}

/// Persist `presets` into the application options.
fn store_presets(presets: Vec<SyncPreset>, cx: &mut App) {
    let mut options = cx.global::<AppOptions>().clone();
    options.sync_presets = presets;
    if let Err(e) = save_options(&options) {
        crate::log::push(
            crate::log::LogLevel::Error,
            format!("Couldn't save the sync presets. {e}"),
            cx,
        );
        return;
    }
    cx.set_global(options);
}

// ---------------------------------------------------------------------------
// Management dialog
// ---------------------------------------------------------------------------

/// The dialog body: the preset list plus an add row.
struct SyncPresetsBody {
    name_input: Entity<InputState>,
    kind: SyncKind,
    presets: Vec<SyncPreset>,
    error: Option<String>,
}

impl SyncPresetsBody {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let presets = cx.global::<AppOptions>().sync_presets.clone();
        Self {
            name_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Name")),
            kind: SyncKind::UpdateRight,
            presets,
            error: None,
        }
    }

    /// Add a preset from the current input; duplicates and blank names are
    /// rejected in place.
    fn add(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a name for the preset.".into());
            cx.notify();
            return;
        }
        if self.presets.iter().any(|p| p.name == name) {
            self.error = Some(format!("A preset named \"{name}\" exists."));
            cx.notify();
            return;
        }
        self.presets.push(SyncPreset {
            name,
            kind: self.kind,
        });
        store_presets(self.presets.clone(), cx);
        self.name_input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.error = None;
        cx.notify();
    }

    /// Remove the preset with the given name.
    fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        self.presets.retain(|p| p.name != name);
        store_presets(self.presets.clone(), cx);
        self.error = None;
        cx.notify();
    }
}

impl Render for SyncPresetsBody {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let kind = self.kind;

        let list =
            if self.presets.is_empty() {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No presets yet. Add one below.")
                    .into_any_element()
            } else {
                v_flex()
                    .gap_1()
                    .children(self.presets.clone().into_iter().map(|preset| {
                        let remove_name = preset.name.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme.muted)
                            .child(div().flex_1().min_w_0().truncate().child(
                                Label::new(preset.name.clone()).text_sm(),
                            ))
                            .child(
                                Label::new(preset.kind.label())
                                    .text_sm()
                                    .text_color(muted),
                            )
                            .child(
                                Button::new(format!(
                                    "preset-remove-{}",
                                    remove_name
                                ))
                                .icon(IconName::Close)
                                .xsmall()
                                .ghost()
                                .tooltip("Remove preset")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove(&remove_name, cx);
                                    }),
                                ),
                            )
                    }))
                    .into_any_element()
            };

        let kind_radios =
            div().flex().flex_wrap().items_center().gap_4().children(
                SyncKind::ALL.iter().copied().map(|option| {
                    Radio::new(format!("preset-kind-{}", option.label()))
                        .label(option.label())
                        .checked(kind == option)
                        .on_change(cx.listener(move |this, checked, _, cx| {
                            if *checked {
                                this.kind = option;
                                cx.notify();
                            }
                        }))
                }),
            );

        v_flex()
            .w_full()
            .gap_3()
            .child(list)
            .child(div().w_full().h_px().bg(theme.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w_48().child(Input::new(&self.name_input)))
                    .child(
                        Button::new("preset-add")
                            .label("Add preset")
                            .primary()
                            .on_click(cx.listener(Self::add)),
                    ),
            )
            .child(kind_radios)
            .when_some(self.error.clone(), |this, err| {
                this.child(Label::new(err).text_sm().text_color(danger))
            })
    }
}

/// Open the folder-sync presets dialog (Options menu).
pub(crate) fn open_sync_presets_dialog(window: &mut Window, cx: &mut App) {
    let body = cx.new(|cx| SyncPresetsBody::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Folder Sync presets")
            .w(px(560.))
            .child(body.clone())
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_through_options() {
        let presets = vec![
            SyncPreset {
                name: "Deploy".into(),
                kind: SyncKind::MirrorRight,
            },
            SyncPreset {
                name: "Pull".into(),
                kind: SyncKind::UpdateLeft,
            },
        ];
        let options = AppOptions {
            sync_presets: presets.clone(),
            ..AppOptions::default()
        };
        let json = serde_json::to_string(&options).expect("serialize");
        let back: AppOptions = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.sync_presets, presets);
    }

    #[test]
    fn missing_presets_field_defaults_empty() {
        // Options files written before presets existed must still load.
        let back: AppOptions =
            serde_json::from_str("{}").expect("parse legacy options");
        assert!(back.sync_presets.is_empty());
    }

    #[test]
    fn preset_action_maps_every_kind() {
        let name = |kind: SyncKind| sync_preset_action(kind).name();
        assert!(name(SyncKind::UpdateRight).ends_with("SyncUpdateRight"));
        assert!(name(SyncKind::UpdateLeft).ends_with("SyncUpdateLeft"));
        assert!(name(SyncKind::UpdateBoth).ends_with("SyncUpdateBoth"));
        assert!(name(SyncKind::MirrorRight).ends_with("SyncMirrorRight"));
        assert!(name(SyncKind::MirrorLeft).ends_with("SyncMirrorLeft"));
    }
}
