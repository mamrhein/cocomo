// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Profile management: the Options ▸ Profiles dialog.
//!
//! Profiles are the durable credential store for remote endpoints: the
//! resolver chain (see [`cocomo_lib::resolver`]) consults the profile
//! store before the environment, the keychain, and the interactive
//! credential dialog ([`crate::credentials`]). This page is a CRUD
//! editor over [`ProfileStore`]: each profile carries a provider type,
//! per-type settings, and an optional password secret. Secrets are
//! stored encrypted by the library (master-key derived); on macOS the
//! keychain remains a second read path at resolution time.

use std::collections::HashMap;

use cocomo_lib::{Profile, ProfileStore, ProviderType};
use gpui::{
    App, ClickEvent, Context, ElementId, Entity, IntoElement, Render,
    SharedString, Window,
    component::{
        ActiveTheme, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        label::Label,
        radio::Radio,
        v_flex,
    },
    div,
    prelude::*,
    px,
};

use crate::log::{self, LogLevel};

/// The provider kinds a profile can configure, in display order. Local
/// endpoints need no profile, so they are not offered.
const KINDS: [ProviderType; 5] = [
    ProviderType::Ftp,
    ProviderType::Sftp,
    ProviderType::S3,
    ProviderType::WebDav,
    ProviderType::Zip,
];

/// One settings field of a provider kind.
struct FieldSpec {
    /// The profile settings key the field writes.
    key: &'static str,
    /// The form label.
    label: &'static str,
}

/// The settings fields of each offered provider kind, in display order.
fn fields_for(kind: ProviderType) -> &'static [FieldSpec] {
    match kind {
        ProviderType::Ftp => &[
            FieldSpec {
                key: "host",
                label: "Host",
            },
            FieldSpec {
                key: "port",
                label: "Port",
            },
            FieldSpec {
                key: "username",
                label: "Username",
            },
            FieldSpec {
                key: "tls",
                label: "TLS (true/false)",
            },
            FieldSpec {
                key: "root_path",
                label: "Root path",
            },
        ],
        ProviderType::Sftp => &[
            FieldSpec {
                key: "host",
                label: "Host",
            },
            FieldSpec {
                key: "port",
                label: "Port",
            },
            FieldSpec {
                key: "username",
                label: "Username",
            },
            FieldSpec {
                key: "key_file",
                label: "Private key file",
            },
            FieldSpec {
                key: "root_path",
                label: "Root path",
            },
        ],
        ProviderType::S3 => &[
            FieldSpec {
                key: "region",
                label: "Region",
            },
            FieldSpec {
                key: "bucket",
                label: "Bucket",
            },
            FieldSpec {
                key: "prefix",
                label: "Prefix",
            },
        ],
        ProviderType::WebDav => &[
            FieldSpec {
                key: "base_url",
                label: "Base URL",
            },
            FieldSpec {
                key: "username",
                label: "Username",
            },
            FieldSpec {
                key: "tls",
                label: "TLS (true/false)",
            },
            FieldSpec {
                key: "root_path",
                label: "Root path",
            },
        ],
        ProviderType::Zip => &[FieldSpec {
            key: "archive",
            label: "Archive file",
        }],
        ProviderType::Local => &[],
    }
}

/// Whether the kind authenticates with a password secret.
fn has_secret(kind: ProviderType) -> bool {
    matches!(
        kind,
        ProviderType::Ftp | ProviderType::Sftp | ProviderType::WebDav
    )
}

/// A short one-line summary of a profile for the list rows.
fn profile_summary(profile: &Profile) -> String {
    let endpoint = ["host", "base_url", "bucket", "archive"]
        .iter()
        .find_map(|key| profile.setting(key))
        .unwrap_or("");
    if endpoint.is_empty() {
        profile.provider_type.to_string()
    } else {
        format!("{} · {}", profile.provider_type, endpoint)
    }
}

/// The dialog body: the profile list plus the editor for the selected
/// profile.
struct ProfilesBody {
    /// The store snapshot shown in the list (never carries decrypted
    /// secrets).
    profiles: Vec<Profile>,
    /// The id of the profile being edited, `None` while creating.
    selected: Option<String>,
    /// Whether the editor holds a not-yet-stored new profile.
    creating: bool,
    provider_type: ProviderType,
    id_input: Entity<InputState>,
    /// Settings inputs keyed by settings key, created per provider kind
    /// on first use.
    field_inputs: HashMap<&'static str, Entity<InputState>>,
    secret_input: Entity<InputState>,
    error: Option<String>,
}

impl ProfilesBody {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            profiles: Vec::new(),
            selected: None,
            creating: false,
            provider_type: ProviderType::Ftp,
            id_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Name")),
            field_inputs: HashMap::new(),
            secret_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            error: None,
        };
        this.reload(cx);
        this
    }

    /// Re-read the profile list from the store.
    fn reload(&mut self, cx: &mut Context<Self>) {
        match ProfileStore::open_default().and_then(|s| s.list()) {
            Ok(profiles) => self.profiles = profiles,
            Err(e) => {
                self.error = Some(format!("Couldn't read the profiles. {e}"));
            }
        }
        cx.notify();
    }

    /// The input for one settings key, created on first use.
    fn field_input(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.field_inputs.get(key).cloned().unwrap_or_else(|| {
            let input = cx.new(|cx| InputState::new(window, cx));
            self.field_inputs.insert(key, input.clone());
            input
        })
    }

    /// Clear the editor into "new profile" mode.
    fn new_profile(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset_form(window, cx);
    }

    /// Clear the editor into "new profile" mode.
    fn reset_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        self.creating = true;
        self.provider_type = ProviderType::Ftp;
        self.error = None;
        self.id_input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        self.secret_input.update(cx, |input, cx| {
            input.set_value(String::new(), window, cx);
        });
        for input in self.field_inputs.values() {
            input.update(cx, |input, cx| {
                input.set_value(String::new(), window, cx);
            });
        }
        cx.notify();
    }

    /// Load a stored profile (with decrypted secrets) into the editor.
    fn select(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = match ProfileStore::open_default() {
            Ok(store) => store,
            Err(e) => {
                self.error = Some(format!("Couldn't open the store. {e}"));
                cx.notify();
                return;
            }
        };
        let profile = match store.get_decrypted(id) {
            Ok(Some(profile)) => profile,
            Ok(None) => {
                self.error = Some(format!("Profile \"{id}\" is gone."));
                self.reload(cx);
                return;
            }
            Err(e) => {
                self.error = Some(format!("Couldn't read \"{id}\". {e}"));
                cx.notify();
                return;
            }
        };
        self.selected = Some(id.to_string());
        self.creating = false;
        self.provider_type = profile.provider_type;
        self.error = None;
        self.id_input.update(cx, |input, cx| {
            input.set_value(id.to_string(), window, cx);
        });
        self.secret_input.update(cx, |input, cx| {
            input.set_value(
                profile.secrets.get("password").unwrap_or_default(),
                window,
                cx,
            );
        });
        for spec in fields_for(profile.provider_type) {
            let value = profile.setting(spec.key).unwrap_or_default();
            let input = self.field_input(spec.key, window, cx);
            input.update(cx, |input, cx| {
                input.set_value(value.to_string(), window, cx);
            });
        }
        cx.notify();
    }

    /// Validate the editor and store the profile (add or update).
    fn save(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.id_input.read(cx).value().trim().to_string();
        if id.is_empty() {
            self.error = Some("Enter a name for the profile.".into());
            cx.notify();
            return;
        }
        let mut profile = Profile::new(id.clone(), self.provider_type);
        for spec in fields_for(self.provider_type) {
            let Some(input) = self.field_inputs.get(spec.key) else {
                continue;
            };
            let value = input.read(cx).value().trim().to_string();
            if !value.is_empty() {
                profile.set_setting(spec.key.to_string(), value);
            }
        }
        if has_secret(self.provider_type) {
            let secret = self.secret_input.read(cx).value().to_string();
            if !secret.is_empty() {
                profile.secrets.set("password".into(), secret);
            }
        }
        let result = ProfileStore::open_default().and_then(|store| {
            if self.creating {
                store.add(profile).map(|_| ())
            } else {
                store.update(profile).map(|_| ())
            }
        });
        if let Err(e) = result {
            self.error = Some(match e {
                cocomo_lib::ProfileError::AlreadyExists(profile) => {
                    format!("A profile named \"{profile}\" exists.")
                }
                other => format!("Couldn't save the profile. {other}"),
            });
            cx.notify();
            return;
        }
        self.error = None;
        self.creating = false;
        self.selected = Some(id);
        self.reload(cx);
    }

    /// Delete the selected profile.
    fn delete(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        match ProfileStore::open_default().and_then(|store| store.remove(&id))
        {
            Ok(_) => {
                log::push(
                    LogLevel::Info,
                    format!("Profile \"{id}\" deleted."),
                    cx,
                );
                self.reset_form(window, cx);
            }
            Err(e) => {
                self.error = Some(format!("Couldn't delete \"{id}\". {e}"));
                cx.notify();
            }
        }
        self.reload(cx);
    }

    /// Switch the provider kind, clearing field values that belong to
    /// no field of the new kind.
    fn set_kind(
        &mut self,
        kind: ProviderType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.provider_type == kind {
            return;
        }
        self.provider_type = kind;
        for input in self.field_inputs.values() {
            input.update(cx, |input, cx| {
                input.set_value(String::new(), window, cx);
            });
        }
        cx.notify();
    }
}

impl Render for ProfilesBody {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        // Copy the tokens up front; the field-input closure below needs
        // `cx` mutably while the theme borrow would still be alive.
        let (muted_bg, accent, stripe, border) =
            (theme.muted, theme.accent, theme.table_even, theme.border);

        // The list rows.
        let list = if self.profiles.is_empty() {
            div()
                .text_sm()
                .text_color(muted)
                .child("No profiles yet. Create one below.")
                .into_any_element()
        } else {
            v_flex()
                .gap_1()
                .children(self.profiles.clone().into_iter().enumerate().map(
                    |(ix, profile)| {
                        let name: SharedString = profile.id.clone().into();
                        let selected = !self.creating
                            && self.selected.as_deref()
                                == Some(profile.id.as_str());
                        let row_id = profile.id.clone();
                        div()
                            .id(ElementId::Name(
                                format!("profile-{name}").into(),
                            ))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(muted_bg))
                            .when(selected, |row| row.bg(accent))
                            .when(ix % 2 == 1 && !selected, |row| {
                                row.bg(stripe)
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(Label::new(name).text_sm()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .flex_shrink_0()
                                    .text_color(muted)
                                    .child(profile_summary(&profile)),
                            )
                            .on_click(cx.listener(
                                move |this, _, window: &mut Window, cx| {
                                    this.select(&row_id, window, cx);
                                },
                            ))
                    },
                ))
                .into_any_element()
        };

        // The editor: name + kind + per-kind fields + secret.
        let kind_radios =
            div().flex().flex_wrap().items_center().gap_4().children(
                KINDS.iter().copied().map(|kind| {
                    Radio::new(format!("profile-kind-{kind}"))
                        .label(kind.to_string())
                        .checked(self.provider_type == kind)
                        .on_change(cx.listener(
                            move |this, checked, window, cx| {
                                if *checked {
                                    this.set_kind(kind, window, cx);
                                }
                            },
                        ))
                }),
            );

        let field_rows = v_flex().gap_2().children(
            fields_for(self.provider_type).iter().map(|spec| {
                let input = self.field_input(spec.key, window, cx);
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w_40()
                            .flex_shrink_0()
                            .child(Label::new(spec.label).text_sm()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&input).small()),
                    )
            }),
        );

        let secret_row = has_secret(self.provider_type).then(|| {
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w_40()
                        .flex_shrink_0()
                        .child(Label::new("Password").text_sm()),
                )
                .child(div().flex_1().min_w_0().child(
                    Input::new(&self.secret_input).small().mask_toggle(),
                ))
        });

        let buttons = h_flex()
            .gap_2()
            .child(
                Button::new("profile-new")
                    .label("New profile")
                    .on_click(cx.listener(Self::new_profile)),
            )
            .child(
                Button::new("profile-save")
                    .label("Save profile")
                    .primary()
                    .on_click(cx.listener(Self::save)),
            )
            .when_some(self.selected.clone(), |this, id| {
                this.child(
                    Button::new("profile-delete")
                        .label(format!("Delete \"{id}\""))
                        .ghost()
                        .on_click(cx.listener(Self::delete)),
                )
            });

        v_flex()
            .w_full()
            .gap_3()
            .child(
                div()
                    .id("profiles-list")
                    .max_h(px(192.))
                    .overflow_y_scroll()
                    .pr_1()
                    .child(list),
            )
            .child(div().w_full().h_px().bg(border))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w_40()
                            .flex_shrink_0()
                            .child(Label::new("Name").text_sm()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.id_input).small()),
                    ),
            )
            .child(kind_radios)
            .child(field_rows)
            .children(secret_row)
            .child(div().text_xs().text_color(muted).child(
                "Passwords are stored encrypted in the profile file; on \
                 macOS the keychain is consulted as well. Credentials \
                 entered into a prompt are kept for the current run only.",
            ))
            .child(buttons)
            .when_some(self.error.clone(), |this, err| {
                this.child(Label::new(err).text_sm().text_color(danger))
            })
    }
}

/// Open the Profiles dialog (Options menu).
pub(crate) fn open_profiles_dialog(window: &mut Window, cx: &mut App) {
    let body = cx.new(|cx| ProfilesBody::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Profiles").w(px(560.)).child(body.clone())
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_kind_has_fields() {
        for kind in KINDS {
            assert!(
                !fields_for(kind).is_empty(),
                "{kind} must offer at least one settings field"
            );
        }
        assert!(fields_for(ProviderType::Local).is_empty());
    }

    #[test]
    fn password_kinds_match_the_resolver_chain() {
        assert!(has_secret(ProviderType::Ftp));
        assert!(has_secret(ProviderType::Sftp));
        assert!(has_secret(ProviderType::WebDav));
        assert!(!has_secret(ProviderType::S3));
        assert!(!has_secret(ProviderType::Zip));
    }

    #[test]
    fn summary_prefers_an_endpoint_setting() {
        let mut profile = Profile::new("p", ProviderType::Ftp);
        assert_eq!(profile_summary(&profile), "ftp");
        profile.set_setting("host".into(), "ftp.example.com".into());
        assert_eq!(profile_summary(&profile), "ftp · ftp.example.com");
    }
}
