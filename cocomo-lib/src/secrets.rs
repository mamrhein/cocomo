// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Credential lookup and interactive credential entry for remote providers.
//!
//! Remote providers (FTP/FTPS, S3, WebDAV) need credentials that must never
//! appear in URLs or on the command line (see the `url` module, OQ1). This
//! module provides the non-profile parts of the credential resolution chain
//! used by `Provider::resolve`:
//!
//! - [`Secrets`]: a read-only password-safe lookup. It checks a
//!   `COCOMO_{SCHEME}_{KEY}` environment variable first (e.g.
//!   `COCOMO_FTP_PASSWORD` for `Secrets::get("ftp", "password")`) and then, on
//!   macOS with the `keychain` feature enabled, the macOS keychain via the
//!   `/usr/bin/security` tool.
//! - [`Prompter`]: an abstraction over interactive, one-time credential entry.
//!   Secrets read through a prompter are used for the current process only and
//!   never persisted. [`NullPrompter`] never prompts (the default for
//!   non-interactive runs and for tests); [`TtyPrompter`] reads from the
//!   terminal when the `prompt` feature is enabled.

use std::env;
#[cfg(feature = "prompt")]
use std::io::{self, BufRead, IsTerminal, Write};
#[cfg(all(feature = "keychain", target_os = "macos"))]
use std::process::Command;

/// A read-only password safe backed by environment variables and, on macOS
/// with the `keychain` feature, the macOS keychain.
#[derive(Clone, Debug)]
pub struct Secrets {
    /// Whether the platform keychain is consulted after the environment.
    #[cfg(all(feature = "keychain", target_os = "macos"))]
    use_keychain: bool,
}

impl Secrets {
    /// Create a password safe honoring the build's feature flags.
    pub fn new() -> Self {
        Self {
            // The keychain backend only exists on macOS and can be turned
            // off entirely via `default-features = false`.
            use_keychain: cfg!(all(feature = "keychain", target_os = "macos")),
        }
    }

    /// Create a password safe with the keychain lookup explicitly enabled or
    /// disabled (mainly useful for tests).
    pub fn with_keychain(enabled: bool) -> Self {
        Self {
            use_keychain: enabled,
        }
    }

    /// Look up the secret stored under `key` for `scheme`.
    ///
    /// Checks the environment variable `COCOMO_{SCHEME}_{KEY}` (both parts
    /// uppercased, e.g. `COCOMO_FTP_PASSWORD` for `("ftp", "password")` and
    /// `COCOMO_S3_ACCESS_KEY` for `("s3", "access_key")`) first, then the
    /// macOS keychain (service `cocomo.{scheme}`, account `key`). Returns
    /// `None` if neither source provides the secret.
    pub fn get(&self, scheme: &str, key: &str) -> Option<String> {
        let env_name = format!(
            "COCOMO_{}_{}",
            scheme.to_ascii_uppercase(),
            key.to_ascii_uppercase()
        );
        if let Ok(value) = env::var(&env_name)
            && !value.is_empty()
        {
            return Some(value);
        }
        self.keychain_get(scheme, key)
    }

    /// Keychain lookup is only available on macOS with the `keychain`
    /// feature; everywhere else this always yields `None`.
    fn keychain_get(&self, service: &str, account: &str) -> Option<String> {
        #[cfg(all(feature = "keychain", target_os = "macos"))]
        {
            if self.use_keychain {
                return keychain_lookup(&format!("cocomo.{service}"), account);
            }
            None
        }
        #[cfg(not(all(feature = "keychain", target_os = "macos")))]
        {
            let _ = (service, account);
            None
        }
    }
}

impl Default for Secrets {
    fn default() -> Self {
        Self::new()
    }
}

/// Read one secret from the macOS keychain via the `security` tool.
///
/// Spawning `/usr/bin/security` keeps the keychain helper free of extra
/// crate dependencies; a miss (or an unavailable tool) yields `None`.
#[cfg(all(feature = "keychain", target_os = "macos"))]
fn keychain_lookup(service: &str, account: &str) -> Option<String> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

/// Interactive, one-time credential entry for a remote endpoint.
pub trait Prompter {
    /// Return whether interactive credential entry is possible at all (i.e.
    /// whether both stdin and stderr are attached to a terminal).
    fn is_tty(&self) -> bool;

    /// Prompt for the username to log in at `endpoint`.
    fn prompt_user(&self, endpoint: &str) -> Option<String>;

    /// Prompt for the secret (password or access key) for `endpoint`.
    fn prompt_secret(&self, endpoint: &str) -> Option<String>;
}

/// A prompter that never prompts: the default for non-interactive runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullPrompter;

impl Prompter for NullPrompter {
    fn is_tty(&self) -> bool {
        false
    }

    fn prompt_user(&self, _endpoint: &str) -> Option<String> {
        None
    }

    fn prompt_secret(&self, _endpoint: &str) -> Option<String> {
        None
    }
}

/// A prompter that reads from the terminal when the `prompt` feature is
/// enabled; without that feature it behaves like [`NullPrompter`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TtyPrompter;

impl TtyPrompter {
    /// Create a new terminal prompter.
    pub fn new() -> Self {
        Self
    }
}

impl Prompter for TtyPrompter {
    #[cfg(feature = "prompt")]
    fn is_tty(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    #[cfg(not(feature = "prompt"))]
    fn is_tty(&self) -> bool {
        false
    }

    #[cfg(feature = "prompt")]
    fn prompt_user(&self, endpoint: &str) -> Option<String> {
        prompt_visible(&format!("Username for {endpoint}: "))
    }

    #[cfg(not(feature = "prompt"))]
    fn prompt_user(&self, _endpoint: &str) -> Option<String> {
        None
    }

    #[cfg(feature = "prompt")]
    fn prompt_secret(&self, endpoint: &str) -> Option<String> {
        prompt_secret(&format!("Password for {endpoint}: "))
    }

    #[cfg(not(feature = "prompt"))]
    fn prompt_secret(&self, _endpoint: &str) -> Option<String> {
        None
    }
}

/// Print `prompt` on stderr and read one visible line from stdin.
///
/// Prompts go to stderr so that piped stdout output of the CLI stays
/// machine-readable.
#[cfg(feature = "prompt")]
fn prompt_visible(prompt: &str) -> Option<String> {
    let mut err = io::stderr();
    write!(err, "{prompt}").ok()?;
    err.flush().ok()?;
    let mut line = Vec::new();
    io::stdin().lock().read_until(b'\n', &mut line).ok()?;
    let line = String::from_utf8_lossy(&line).trim_end().to_owned();
    if line.is_empty() { None } else { Some(line) }
}

/// Read one hidden line from the terminal (never echoed, never persisted).
#[cfg(feature = "prompt")]
fn prompt_secret(prompt: &str) -> Option<String> {
    let secret = rpassword::prompt_password(prompt).ok()?;
    // A bare Enter means "no secret", mirroring `prompt_visible`: an empty
    // password would only be rejected by the server, while `None` lets the
    // key-based fallbacks in `SftpFs::authenticate` run.
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_from_env_var() {
        // `with_keychain(false)` keeps this test independent of whatever
        // the developer's keychain happens to contain.
        let secrets = Secrets::with_keychain(false);
        temp_env::with_var("COCOMO_FTP_PASSWORD", Some("envpass"), || {
            assert_eq!(
                secrets.get("ftp", "password").as_deref(),
                Some("envpass")
            );
        });
    }

    #[test]
    fn empty_env_var_is_not_a_secret() {
        let secrets = Secrets::with_keychain(false);
        temp_env::with_var("COCOMO_FTP_PASSWORD", Some(""), || {
            assert_eq!(secrets.get("ftp", "password"), None);
        });
    }

    #[test]
    fn missing_secret_stays_none() {
        let secrets = Secrets::new();
        // Neither the (disabled or unsupported) keychain nor the
        // environment provides an entry for this made-up service.
        assert_eq!(secrets.get("cocomotest", "cocomotest-secret"), None);
    }
}
