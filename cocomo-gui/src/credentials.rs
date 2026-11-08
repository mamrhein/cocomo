// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Dialog-backed credential entry for remote endpoints.
//!
//! The library resolves remote credentials through a chain (profile
//! store, environment, keychain, then [`Prompter`]). The GUI has no
//! terminal, so a prompt must be a window dialog — and a dialog cannot
//! block the main thread the resolver runs on. This module therefore
//! splits prompting in two:
//!
//! 1. Resolution runs against [`DialogPrompter`], which only ever answers from
//!    the process-only [`CredentialCache`]. On the first attempt the cache is
//!    empty, so resolution fails with `ProviderError::AuthRequired`.
//! 2. [`resolve_remote`] catches that error, opens a credential dialog for the
//!    endpoint, and — once the user supplies credentials — stores them in the
//!    cache and re-runs the caller's open operation through the `retry`
//!    callback. The second resolution finds the cached credentials and
//!    succeeds.
//!
//! Cached credentials live for the current process only and are never
//! persisted, mirroring the library's prompter contract. Profiles and
//! the keychain remain the durable paths (see [`crate::profiles`]).

use std::{
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use anyhow::anyhow;
use cocomo_lib::{Endpoint, Prompter, ProviderError, ResolverError, Url};
use gpui::{
    App, AppContext as _,
    component::{
        ActiveTheme, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div,
    prelude::*,
};

use crate::log::{self, LogLevel};

// ---------------------------------------------------------------------------
// Process-only credential cache
// ---------------------------------------------------------------------------

/// One endpoint's interactively supplied credentials.
#[derive(Clone, Debug)]
struct CachedCredential {
    user: Option<String>,
    secret: String,
}

/// Credentials entered into the credential dialog, keyed by endpoint
/// authority (`scheme://host[:port]`).
///
/// A process-wide static rather than a gpui global because the
/// [`Prompter`] trait methods run without an `App` context; resolution
/// and dialog callbacks both run on the main thread, so a plain mutex
/// suffices.
static CREDENTIALS: OnceLock<Mutex<HashMap<String, CachedCredential>>> =
    OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CachedCredential>> {
    CREDENTIALS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cache key for a resolver endpoint string: the URL's authority,
/// or the raw string when it carries no scheme or does not parse.
fn authority_of(endpoint: &str) -> String {
    // Without an explicit scheme, `Url::parse` reads the string as a
    // local path, which would collapse every bare host or path onto one
    // `file://` key; keep such endpoints verbatim.
    if !endpoint.contains("://") {
        return endpoint.to_string();
    }
    match Url::parse(endpoint) {
        Ok(url) => {
            let host = url.host.clone().unwrap_or_default();
            let host = if host.contains(':') {
                format!("[{host}]")
            } else {
                host
            };
            match url.port {
                Some(port) => {
                    format!("{}://{host}:{port}", url.scheme)
                }
                None => format!("{}://{host}", url.scheme),
            }
        }
        Err(_) => endpoint.to_string(),
    }
}

fn cached(endpoint: &str) -> Option<CachedCredential> {
    cache().lock().ok()?.get(&authority_of(endpoint)).cloned()
}

/// Remember interactively entered credentials for `endpoint` for the
/// rest of this process run.
pub(crate) fn store_credentials(
    endpoint: &str,
    user: Option<String>,
    secret: String,
) {
    if let Ok(mut map) = cache().lock() {
        map.insert(authority_of(endpoint), CachedCredential { user, secret });
    }
}

#[cfg(test)]
fn clear_credentials() {
    if let Ok(mut map) = cache().lock() {
        map.clear();
    }
}

// ---------------------------------------------------------------------------
// Prompter
// ---------------------------------------------------------------------------

/// A prompter that answers from the credential dialog's cache.
///
/// `is_tty` reports "interactive entry is possible" exactly when the
/// dialog has already supplied credentials for the endpoint, so the
/// first resolution attempt fails fast with `AuthRequired` (no network
/// probe, no prompt) and the retry after the dialog succeeds.
pub(crate) struct DialogPrompter;

impl Prompter for DialogPrompter {
    fn is_tty(&self, endpoint: &str) -> bool {
        cached(endpoint).is_some()
    }

    fn prompt_user(&self, endpoint: &str) -> Option<String> {
        cached(endpoint).and_then(|c| c.user)
    }

    fn prompt_secret(&self, endpoint: &str) -> Option<String> {
        let secret = cached(endpoint)?.secret;
        (!secret.is_empty()).then_some(secret)
    }
}

// ---------------------------------------------------------------------------
// Resolution with dialog fallback
// ---------------------------------------------------------------------------

/// The outcome of resolving a remote endpoint through
/// [`resolve_remote`].
pub(crate) enum RemoteOpen {
    /// The endpoint resolved.
    Resolved(Endpoint),
    /// A credential dialog is open; `retry` re-runs the open once the
    /// user supplies credentials. The caller must treat the operation
    /// as accepted, not failed.
    Prompted,
    /// Resolution failed for a non-credential reason.
    Failed(anyhow::Error),
}

/// Resolve a saved-session provider ref, opening a credential dialog
/// when the resolution chain cannot supply credentials.
///
/// On `AuthRequired` the dialog is opened for the endpoint and `retry`
/// is invoked after the user confirms, re-running the whole open with
/// the cache warm. `retry` must be cheap to re-run (re-opening a tab).
pub(crate) fn resolve_remote(
    provider: &str,
    path: PathBuf,
    what: &str,
    retry: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) -> RemoteOpen {
    match Endpoint::from_ref(provider, path, &DialogPrompter) {
        Ok(endpoint) => RemoteOpen::Resolved(endpoint),
        Err(ResolverError::Provider(ProviderError::AuthRequired {
            endpoint,
        })) => {
            if cx.active_window().is_none() {
                return RemoteOpen::Failed(anyhow!(
                    "authentication required for {endpoint}; open a window \
                     to enter credentials"
                ));
            }
            open_credential_dialog(endpoint, what.to_string(), retry, cx);
            RemoteOpen::Prompted
        }
        Err(e) => RemoteOpen::Failed(e.into()),
    }
}

/// Ask for the credentials `endpoint` needs. Confirming caches them and
/// runs `retry`; cancelling reports the pending failure in the log.
fn open_credential_dialog(
    endpoint: String,
    what: String,
    retry: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let Ok((user_input, secret_input)) = window.update(cx, |_, window, cx| {
        let user_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Username (optional)")
        });
        let secret_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        (user_input, secret_input)
    }) else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        window.open_dialog(cx, move |dialog, _window, cx| {
            let user_ok = user_input.clone();
            let secret_ok = secret_input.clone();
            let ok_endpoint = endpoint.clone();
            let retry_ok = retry.clone();
            let cancel_endpoint = endpoint.clone();
            dialog
                .title("Credentials")
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{what} needs credentials for \
                                     {endpoint}. They are kept for this run \
                                     only.",
                                )),
                        )
                        .child(Input::new(&user_input))
                        .child(Input::new(&secret_input).mask_toggle()),
                )
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("credentials_cancel")
                                .label("Cancel")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    log::push(
                                        LogLevel::Warning,
                                        format!(
                                            "No credentials entered for \
                                             {cancel_endpoint}."
                                        ),
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("credentials_ok")
                                .label("Connect")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let secret =
                                        secret_ok.read(cx).value().to_string();
                                    if secret.trim().is_empty() {
                                        return;
                                    }
                                    let user = user_ok
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .to_string();
                                    window.close_dialog(cx);
                                    store_credentials(
                                        &ok_endpoint,
                                        (!user.is_empty()).then_some(user),
                                        secret,
                                    );
                                    retry_ok(cx);
                                }),
                        ),
                )
        });
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_normalizes_the_endpoint_key() {
        assert_eq!(
            authority_of("ftp://host.example/pub"),
            "ftp://host.example"
        );
        assert_eq!(
            authority_of("sftp://host.example:2222/data"),
            "sftp://host.example:2222"
        );
        // A non-URL endpoint string is its own key.
        assert_eq!(authority_of("host.example"), "host.example");
    }

    #[test]
    fn prompter_answers_only_after_credentials_are_stored() {
        clear_credentials();
        let endpoint = "ftp://prompt-test.example/pub";
        assert!(!DialogPrompter.is_tty(endpoint));
        assert_eq!(DialogPrompter.prompt_secret(endpoint), None);

        store_credentials(endpoint, Some("alice".into()), "s3cret".into());
        // A different path on the same authority hits the same entry.
        assert!(DialogPrompter.is_tty("ftp://prompt-test.example/other"));
        assert_eq!(
            DialogPrompter.prompt_user(endpoint).as_deref(),
            Some("alice")
        );
        assert_eq!(
            DialogPrompter.prompt_secret(endpoint).as_deref(),
            Some("s3cret")
        );
        clear_credentials();
    }

    #[test]
    fn an_empty_secret_is_not_a_credential() {
        clear_credentials();
        store_credentials("ftp://empty-test.example/p", None, String::new());
        assert_eq!(
            DialogPrompter.prompt_secret("ftp://empty-test.example"),
            None
        );
        clear_credentials();
    }
}
