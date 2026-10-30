// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! File format registry (UI spec section 7.2).
//!
//! Maps a file name to the view kind used to open it: the text compare
//! view today, and the table, hex, or picture views as they arrive.
//! The registry is a global with sensible defaults and is user-extensible
//! through the Tools menu ("File formats...").

use std::path::Path;

use gpui::{
    App, AppContext, ClickEvent, Context, Entity, Global, IntoElement, Render,
    Window,
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
};

/// The kind of view a file format routes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FileViewKind {
    /// Side-by-side text compare view.
    Text,
    /// Table compare view (planned).
    Table,
    /// Hex compare view (planned).
    Hex,
    /// Picture compare view (planned).
    Picture,
}

impl FileViewKind {
    /// All kinds, in display order.
    const ALL: [Self; 4] = [Self::Text, Self::Table, Self::Hex, Self::Picture];

    /// Human-readable name shown in the registry dialog.
    fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Table => "Table",
            Self::Hex => "Hex",
            Self::Picture => "Picture",
        }
    }

    /// Whether a view of this kind can be opened today.
    pub(crate) fn is_available(self) -> bool {
        matches!(self, Self::Text)
    }
}

/// A named set of extensions routed to one view kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileFormat {
    /// Display name of the format.
    pub(crate) name: String,
    /// Lowercase extensions without the leading dot.
    pub(crate) extensions: Vec<String>,
    /// The view kind files of this format open in.
    pub(crate) view: FileViewKind,
}

/// Global registry of file formats.
#[derive(Clone)]
pub(crate) struct FileFormatRegistry {
    formats: Vec<FileFormat>,
}

struct GlobalFileFormats(FileFormatRegistry);

impl Global for GlobalFileFormats {}

impl FileFormatRegistry {
    /// The registry with the built-in defaults.
    fn defaults() -> Self {
        let text = [
            "txt", "md", "markdown", "rst", "adoc", "rs", "py", "js", "ts",
            "jsx", "tsx", "go", "java", "kt", "c", "h", "cpp", "hpp", "cs",
            "rb", "php", "swift", "sh", "bash", "zsh", "fish", "bat", "ps1",
            "html", "htm", "css", "scss", "xml", "json", "yaml", "yml",
            "toml", "ini", "conf", "cfg", "log", "sql", "tex",
        ];
        let table = ["csv", "tsv"];
        let hex = [
            "bin", "exe", "dll", "so", "dylib", "o", "a", "lib", "class",
            "jar", "wasm", "pdf", "zip", "gz", "bz2", "xz", "7z",
        ];
        let picture = [
            "jpg", "jpeg", "gif", "bmp", "webp", "svg", "tiff", "png", "ico",
        ];
        Self {
            formats: vec![
                FileFormat {
                    name: "Text".into(),
                    extensions: text.iter().map(|s| s.to_string()).collect(),
                    view: FileViewKind::Text,
                },
                FileFormat {
                    name: "Table".into(),
                    extensions: table.iter().map(|s| s.to_string()).collect(),
                    view: FileViewKind::Table,
                },
                FileFormat {
                    name: "Hex".into(),
                    extensions: hex.iter().map(|s| s.to_string()).collect(),
                    view: FileViewKind::Hex,
                },
                FileFormat {
                    name: "Picture".into(),
                    extensions: picture
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    view: FileViewKind::Picture,
                },
            ],
        }
    }

    /// Install the default registry as an application global.
    pub(crate) fn init_global(cx: &mut App) {
        cx.set_global(GlobalFileFormats(Self::defaults()));
    }

    /// The view kind for a path. Unknown extensions fall back to the text
    /// view, mirroring how a compare tool treats unrecognized files.
    pub(crate) fn view_kind(path: &Path, cx: &App) -> FileViewKind {
        let Some(ext) =
            path.extension().map(|e| e.to_string_lossy().to_lowercase())
        else {
            return FileViewKind::Text;
        };
        for format in cx.global::<GlobalFileFormats>().0.formats.iter() {
            if format.extensions.contains(&ext) {
                return format.view;
            }
        }
        FileViewKind::Text
    }

    /// All registered formats.
    pub(crate) fn all(cx: &App) -> Vec<FileFormat> {
        cx.global::<GlobalFileFormats>().0.formats.clone()
    }

    /// Add a format, merging extensions into an existing format of the same
    /// name. Returns an error message when the name or an extension is
    /// already taken by a different format.
    pub(crate) fn add_or_update(
        name: &str,
        extensions: &[String],
        view: FileViewKind,
        cx: &mut App,
    ) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("Format name is required.".into());
        }
        if extensions.is_empty() {
            return Err("At least one extension is required.".into());
        }
        let registry = &cx.global::<GlobalFileFormats>().0;
        for ext in extensions {
            if registry.formats.iter().any(|f| {
                f.name != name && f.extensions.iter().any(|e| e == ext)
            }) {
                return Err(format!(
                    "Extension \"{ext}\" is already registered."
                ));
            }
        }
        cx.update_global::<GlobalFileFormats, _>(|global, _| {
            let registry = &mut global.0;
            if let Some(existing) =
                registry.formats.iter_mut().find(|f| f.name == name)
            {
                existing.view = view;
                for ext in extensions {
                    if !existing.extensions.contains(ext) {
                        existing.extensions.push(ext.clone());
                    }
                }
            } else {
                registry.formats.push(FileFormat {
                    name: name.to_string(),
                    extensions: extensions.to_vec(),
                    view,
                });
            }
        });
        Ok(())
    }

    /// Remove a registered format by name.
    pub(crate) fn remove(name: &str, cx: &mut App) {
        cx.update_global::<GlobalFileFormats, _>(|global, _| {
            global.0.formats.retain(|f| f.name != name);
        });
    }
}

/// Parse a comma-separated extension list into normalized, deduplicated
/// extensions.
fn parse_extensions(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in input.split(',') {
        let ext = part.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() && !out.contains(&ext) {
            out.push(ext);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Management dialog
// ---------------------------------------------------------------------------

/// Body of the "File formats" dialog: the format list plus an add form.
/// A `Render` entity so the list refreshes as formats are added or removed
/// (dialog content is otherwise built only once when the dialog opens).
struct FileFormatsBody {
    name_input: Entity<InputState>,
    ext_input: Entity<InputState>,
    view_kind: FileViewKind,
    error: Option<String>,
}

impl FileFormatsBody {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            name_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Name")),
            ext_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Extensions (e.g. sql, ddl)")
            }),
            view_kind: FileViewKind::Text,
            error: None,
        }
    }
}

impl Render for FileFormatsBody {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let view_kind = self.view_kind;

        let list = v_flex().gap_1().children(
            FileFormatRegistry::all(cx).into_iter().map(|format| {
                let remove_name = format.name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.muted)
                    .child(
                        div()
                            .w_32()
                            .flex_shrink_0()
                            .child(Label::new(format.name).text_sm()),
                    )
                    .child(
                        div().flex_1().min_w_0().overflow_hidden().child(
                            Label::new(format.extensions.join(", "))
                                .text_sm()
                                .text_color(muted)
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(format.view.label())
                            .text_sm()
                            .text_color(muted),
                    )
                    .child(
                        Button::new(format!("fmt-remove-{remove_name}"))
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("Remove format")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                FileFormatRegistry::remove(&remove_name, cx);
                                this.error = None;
                                cx.notify();
                            })),
                    )
            }),
        );

        let radios = v_flex().gap_1().child(
            div().flex().items_center().gap_4().children(
                FileViewKind::ALL.into_iter().map(|kind| {
                    Radio::new(format!("fmt-view-{}", kind.label()))
                        .label(kind.label())
                        .checked(view_kind == kind)
                        .on_change(cx.listener(move |this, checked, _, cx| {
                            if *checked {
                                this.view_kind = kind;
                                cx.notify();
                            }
                        }))
                }),
            ),
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
                    .child(div().w_32().child(Input::new(&self.name_input)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.ext_input)),
                    )
                    .child(
                        Button::new("fmt-add")
                            .label("Add")
                            .primary()
                            .on_click(cx.listener(Self::add_with_window)),
                    ),
            )
            .child(radios)
            .when_some(self.error.clone(), |this, err| {
                this.child(Label::new(err).text_sm().text_color(danger))
            })
            .into_any_element()
    }
}

impl FileFormatsBody {
    /// Click shim so `add` can clear the inputs with the click's window.
    fn add_with_window(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.name_input.read(cx).value();
        let exts = parse_extensions(&self.ext_input.read(cx).value());
        match FileFormatRegistry::add_or_update(
            &name,
            &exts,
            self.view_kind,
            cx,
        ) {
            Ok(()) => {
                self.name_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                self.ext_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                self.error = None;
                cx.notify();
            }
            Err(err) => {
                self.error = Some(err);
                cx.notify();
            }
        }
    }
}

/// Open the "File formats" management dialog (Tools menu).
pub(crate) fn open_file_formats_dialog(window: &mut Window, cx: &mut App) {
    let body = cx.new(|cx| FileFormatsBody::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("File formats")
            .w(gpui::px(620.))
            .child(body.clone())
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn view_kind_uses_defaults(cx: &mut App) {
        FileFormatRegistry::init_global(cx);
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/readme.md"), cx),
            FileViewKind::Text
        );
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/data.CSV"), cx),
            FileViewKind::Table
        );
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/app.exe"), cx),
            FileViewKind::Hex
        );
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/photo.JPG"), cx),
            FileViewKind::Picture
        );
        // Unknown extensions fall back to the text view.
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/mystery.qqq"), cx),
            FileViewKind::Text
        );
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("noext"), cx),
            FileViewKind::Text
        );
    }

    #[gpui::test]
    fn user_formats_extend_the_registry(cx: &mut App) {
        FileFormatRegistry::init_global(cx);
        // "sql" is a built-in text extension; claiming it under a new name
        // must fail because it belongs to another format.
        let err = FileFormatRegistry::add_or_update(
            "Scripts",
            &["sql".into()],
            FileViewKind::Text,
            cx,
        );
        assert!(err.is_err(), "duplicate extension must be rejected");
        FileFormatRegistry::add_or_update(
            "Config",
            &["env".into()],
            FileViewKind::Text,
            cx,
        )
        .expect("add Config");
        assert_eq!(
            FileFormatRegistry::view_kind(Path::new("a/app.env"), cx),
            FileViewKind::Text
        );
        FileFormatRegistry::remove("Config", cx);
        assert!(
            !FileFormatRegistry::all(cx)
                .iter()
                .any(|f| f.name == "Config")
        );
    }

    #[gpui::test]
    fn add_rejects_blank_input(cx: &mut App) {
        FileFormatRegistry::init_global(cx);
        assert!(
            FileFormatRegistry::add_or_update(
                " ",
                &["x".into()],
                FileViewKind::Text,
                cx
            )
            .is_err()
        );
        assert!(
            FileFormatRegistry::add_or_update(
                "Empty",
                &[],
                FileViewKind::Text,
                cx
            )
            .is_err()
        );
    }

    #[gpui::test]
    fn same_name_merges_extensions(cx: &mut App) {
        FileFormatRegistry::init_global(cx);
        FileFormatRegistry::add_or_update(
            "Config",
            &["env".into()],
            FileViewKind::Text,
            cx,
        )
        .expect("first add");
        FileFormatRegistry::add_or_update(
            "Config",
            &["env".into(), "profile".into()],
            FileViewKind::Text,
            cx,
        )
        .expect("merge add");
        let config = FileFormatRegistry::all(cx)
            .into_iter()
            .find(|f| f.name == "Config")
            .expect("Config format");
        assert_eq!(config.extensions, vec!["env", "profile"]);
    }

    #[test]
    fn parses_extension_lists() {
        assert_eq!(
            parse_extensions(" SQL, .ddl ,ddl,,"),
            vec!["sql".to_string(), "ddl".to_string()]
        );
        assert!(parse_extensions("  ").is_empty());
    }
}
