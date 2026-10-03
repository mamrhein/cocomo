// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! A parsed, credential-free remote-filesystem URL.
//!
//! This module turns user-supplied path-like arguments (CLI arguments, TUI
//! input fields, session files) into a structured [`Url`] that records the
//! scheme, host, port, and in-provider path needed to construct a provider.
//!
//! # URL syntax
//!
//! - `ftp://host[:port]/path` and `ftps://host[:port]/path` (TLS implied by
//!   `ftps`).
//! - `sftp://host[:port]/path` — SSH file transfer (the transport is encrypted
//!   by SSH itself, so there is no TLS variant).
//! - `s3://bucket[/prefix…]/path` — the bucket name occupies the authority
//!   position, the remainder is the path within the bucket.
//! - `webdav://host[:port]/path` and `webdavs://…` (TLS implied by `webdavs`).
//! - `file:///abs/path`, `file://./rel/path`, and bare paths — local paths.
//!
//! Anything that looks like a scheme (a `://` separator) but is not one of
//! the accepted forms fails to parse, so a mistyped remote URL never
//! silently resolves to a local path.
//!
//! # Credentials
//!
//! URLs never carry credentials. Input that embeds `user:pass@` userinfo is
//! rejected rather than silently stripped, because stripping a password
//! would hide a likely mistake; secrets belong in a profile or the
//! password-safe, not in argv.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use thiserror::Error;

/// Errors that can occur while parsing a URL or bare path.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum UrlError {
    /// The input was empty.
    #[error("empty URL")]
    Empty,

    /// The input uses a scheme other than `file`, `ftp`, `ftps`, `sftp`,
    /// `s3`, `webdav`, or `webdavs`.
    #[error("unsupported URL scheme \"{scheme}\" in \"{url}\"")]
    UnsupportedScheme {
        /// The offending URL as typed by the user.
        url: String,
        /// The unprefixed scheme that was found.
        scheme: String,
    },

    /// The authority contains `user:pass@` userinfo.
    ///
    /// Credentials are rejected, never silently stripped: a URL with
    /// embedded secrets is a likely mistake, and accepting it would leak
    /// secrets through process listings and logs.
    #[error(
        "URL \"{url}\" contains credentials; put secrets in a profile, not \
         in the URL"
    )]
    CredentialsInUrl {
        /// The offending URL as typed by the user.
        url: String,
    },

    /// The URL has no host before the first `/` (or ends right after the
    /// authority).
    #[error("URL \"{url}\" has no host")]
    MissingHost {
        /// The offending URL as typed by the user.
        url: String,
    },

    /// The host part is syntactically invalid (unparsable port, unterminated
    /// IPv6 literal, or an unexpected host in a `file://` URL).
    #[error("invalid host \"{host}\" in URL \"{url}\"")]
    InvalidHost {
        /// The offending URL as typed by the user.
        url: String,
        /// The host part that could not be accepted.
        host: String,
    },

    /// The port is not a number in `0..=65535`.
    #[error("invalid port \"{port}\" in URL \"{url}\"")]
    InvalidPort {
        /// The offending URL as typed by the user.
        url: String,
        /// The port text that appeared after the `:`.
        port: String,
    },
}

/// A parsed, credential-free reference to a filesystem location.
///
/// A `Url` only locates a filesystem: the scheme, authority, and in-provider
/// path. Construction of an actual provider from a `Url` happens separately
/// (see `Provider::from_url`), so this type stays a pure value with no I/O
/// and no secrets.
///
/// ```
/// use cocomo_lib::Url;
///
/// let url = Url::parse("ftp://ftp.example.com/pub/src").unwrap();
/// assert_eq!(url.scheme, "ftp");
/// assert_eq!(url.host.as_deref(), Some("ftp.example.com"));
/// assert_eq!(url.path.as_os_str(), "pub/src");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Url {
    /// URL scheme, one of `file`, `ftp`, `ftps`, `sftp`, `s3`, `webdav`,
    /// `webdavs`.
    pub scheme: String,
    /// Authority host (or S3 bucket name). `None` for local URLs.
    pub host: Option<String>,
    /// Explicit port, if the URL gave one.
    pub port: Option<u16>,
    /// Path within the provider. For remote URLs this is the part after the
    /// authority, relative to the provider root (a leading `/` is dropped so
    /// that S3 keys and FTP/WebDAV paths behave uniformly). For local URLs
    /// this is the whole path.
    pub path: PathBuf,
}

impl Url {
    /// The scheme common to all local URLs.
    pub const LOCAL_SCHEME: &'static str = "file";

    /// The port a remote scheme uses when the URL does not name one.
    pub const DEFAULT_PORTS: &'static [(&'static str, u16)] = &[
        ("ftp", 21),
        ("ftps", 990),
        ("sftp", 22),
        ("webdav", 80),
        ("webdavs", 443),
    ];

    /// Parse a URL or bare path.
    ///
    /// Inputs without a `://` separator are treated as local paths, so a
    /// bare path yields a `file` URL with an empty host. The empty string is
    /// rejected, and parsing never panics.
    ///
    /// # Errors
    ///
    /// Returns a [`UrlError`] if the input is empty, uses an unsupported
    /// scheme, embeds `user:pass@` userinfo, lacks a host, or gives an
    /// invalid port.
    pub fn parse(input: &str) -> Result<Self, UrlError> {
        if input.is_empty() {
            return Err(UrlError::Empty);
        }
        match input.split_once("://") {
            None => Ok(Self::from_path(input)),
            Some((scheme, rest)) => Self::parse_url(scheme, rest, input),
        }
    }

    /// Build a `file` URL from any path (Windows drive-letter paths
    /// supported).
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            scheme: Self::LOCAL_SCHEME.to_owned(),
            host: None,
            port: None,
            path: path.as_ref().to_owned(),
        }
    }

    /// Return the effective port for this URL: the explicit one if given,
    /// otherwise the default port of the scheme, if that scheme has one.
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| {
            Self::DEFAULT_PORTS
                .iter()
                .find(|(scheme, _)| *scheme == self.scheme)
                .map(|&(_, port)| port)
        })
    }

    /// Return whether the scheme implies TLS by default (`ftps`, `webdavs`).
    ///
    /// A matching profile's `tls` key can override this default; the scheme
    /// only sets the fallback.
    pub fn uses_tls_by_default(&self) -> bool {
        matches!(self.scheme.as_str(), "ftps" | "webdavs")
    }

    /// Key used for profile-store and password-safe lookups, e.g.
    /// `ftp:ftp.example.com:21` or `s3:my-bucket`.
    ///
    /// An explicit port is part of the key; local URLs always map to the
    /// bare `file` scheme.
    pub fn lookup_key(&self) -> String {
        match &self.host {
            Some(host) => match self.port {
                Some(port) => {
                    format!("{}:{host}:{port}", self.scheme)
                }
                None => format!("{}:{host}", self.scheme),
            },
            None => self.scheme.clone(),
        }
    }

    /// Parse the scheme and remainder of a URL that contains `://`.
    fn parse_url(
        scheme: &str,
        rest: &str,
        input: &str,
    ) -> Result<Self, UrlError> {
        let scheme = scheme.to_ascii_lowercase();
        if scheme == Self::LOCAL_SCHEME {
            return Self::parse_file_url(scheme, rest, input);
        }
        if !matches!(
            scheme.as_str(),
            "ftp" | "ftps" | "sftp" | "s3" | "webdav" | "webdavs"
        ) {
            return Err(UrlError::UnsupportedScheme {
                url: input.to_owned(),
                scheme: scheme.to_owned(),
            });
        }
        // Split the authority (host or bucket, optional port) from the path
        // at the first `/`.
        let (authority, remainder) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            // Anything with userinfo is refused outright (see OQ1).
            return Err(UrlError::CredentialsInUrl {
                url: input.to_owned(),
            });
        }
        if authority.is_empty() {
            return Err(UrlError::MissingHost {
                url: input.to_owned(),
            });
        }
        let (host, port) = Self::split_authority(authority, input)?;
        if host.is_empty() {
            return Err(UrlError::MissingHost {
                url: input.to_owned(),
            });
        }
        // Drop the leading `/` so the path is always relative to the
        // provider root, no matter which backend the URL addresses.
        let path_str = remainder.strip_prefix('/').unwrap_or(remainder);
        Ok(Self {
            scheme,
            host: Some(host.to_owned()),
            port,
            path: PathBuf::from(path_str),
        })
    }

    /// Parse a `file://` URL: everything after the scheme separator is a
    /// path, or the URL is rejected.
    ///
    /// A nonempty authority that is not a `.`/`..` prefix or a drive-letter
    /// path is refused instead of being guessed at, because either reading
    /// (host, or relative path beginning with the authority text) would
    /// silently resolve to a wrong local path.
    fn parse_file_url(
        scheme: String,
        rest: &str,
        input: &str,
    ) -> Result<Self, UrlError> {
        // A leading `/` or `\` marks an absolute path (the `\` form also
        // covers unc paths like `file://\\server\share`); a leading `.` or
        // `..` is a relative path, and anything else that looks like an
        // authority is refused below.
        let looks_like_path = rest.is_empty()
            || rest.starts_with('/')
            || rest.starts_with(r"\\")
            || rest == "."
            || rest == ".."
            || rest.starts_with("./")
            || rest.starts_with("../");
        let looks_like_drive = is_drive_path(rest);
        if looks_like_path || looks_like_drive {
            return Ok(Self {
                scheme,
                host: None,
                port: None,
                path: PathBuf::from(rest),
            });
        }
        let host = rest.split('/').next().unwrap_or(rest).to_owned();
        Err(UrlError::InvalidHost {
            url: input.to_owned(),
            host: host.to_owned(),
        })
    }

    /// Split an authority into host and optional port.
    ///
    /// Bracketed IPv6 literals (`[fe80::1]:21`) are unwrapped so the colons
    /// inside the literal are not mistaken for a port separator.
    fn split_authority(
        authority: &str,
        input: &str,
    ) -> Result<(String, Option<u16>), UrlError> {
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            // Bracketed IPv6 literal: the colons inside the brackets are
            // part of the host, so the port separator follows the `]`.
            let Some(end) = rest.find(']') else {
                return Err(UrlError::InvalidHost {
                    url: input.to_owned(),
                    host: authority.to_owned(),
                });
            };
            let host = &rest[..end];
            match &rest[end + 1..] {
                "" => (host.to_owned(), None),
                after => {
                    let port_text =
                        after.strip_prefix(':').unwrap_or(after).to_owned();
                    let port = port_text.parse::<u16>().map_err(|_| {
                        UrlError::InvalidPort {
                            url: input.to_owned(),
                            port: port_text.clone(),
                        }
                    })?;
                    (host.to_owned(), Some(port))
                }
            }
        } else {
            match authority.split_once(':') {
                Some((host, port)) => {
                    let port = port.parse::<u16>().map_err(|_| {
                        UrlError::InvalidPort {
                            url: input.to_owned(),
                            port: port.to_owned(),
                        }
                    })?;
                    (host.to_owned(), Some(port))
                }
                None => (authority.to_owned(), None),
            }
        };
        Ok((host, port))
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Some(host) => {
                write!(f, "{}://{}", self.scheme, host)?;
                if let Some(port) = self.port {
                    write!(f, ":{port}")?;
                }
                if !self.path.as_os_str().is_empty() {
                    write!(f, "/{}", self.path.display())?;
                }
                Ok(())
            }
            None => write!(f, "{}", self.path.display()),
        }
    }
}

/// Return whether a `file://` remainder starts with a Windows drive-letter
/// prefix such as `C:/` or `C:\`.
fn is_drive_path(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && (bytes[1] == b':' || bytes[1] == b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_input() {
        assert_eq!(Url::parse(""), Err(UrlError::Empty));
    }

    #[test]
    fn parse_ftp_url() {
        let url = Url::parse("ftp://ftp.example.com:21/pub/src").unwrap();
        assert_eq!(url.scheme, "ftp");
        assert_eq!(url.host.as_deref(), Some("ftp.example.com"));
        assert_eq!(url.port, Some(21));
        assert_eq!(url.path, Path::new("pub/src"));
        // A parsed remote url renders back exactly as it was typed.
        assert_eq!(url.to_string(), "ftp://ftp.example.com:21/pub/src");
    }

    #[test]
    fn parse_ftp_without_port() {
        let url = Url::parse("ftp://files.example.com/mirror").unwrap();
        assert_eq!(url.effective_port(), Some(21));
        assert_eq!(url.port, None);
    }

    #[test]
    fn parse_ftps_sets_tls_default() {
        let url = Url::parse("ftps://secure.example.com/pub").unwrap();
        assert_eq!(url.scheme, "ftps");
        assert!(url.uses_tls_by_default());
        assert_eq!(url.effective_port(), Some(990));
        let plain = Url::parse("ftp://files.example.com/pub").unwrap();
        assert!(!plain.uses_tls_by_default());
    }

    #[test]
    fn parse_sftp_url() {
        let url = Url::parse("sftp://files.example.com:2222/pub").unwrap();
        assert_eq!(url.scheme, "sftp");
        assert_eq!(url.host.as_deref(), Some("files.example.com"));
        assert_eq!(url.port, Some(2222));
        assert_eq!(url.path, Path::new("pub"));
        // SSH encrypts its own transport, so no TLS default is implied.
        assert!(!url.uses_tls_by_default());
        let no_port = Url::parse("sftp://files.example.com/pub").unwrap();
        assert_eq!(no_port.effective_port(), Some(22));
    }

    #[test]
    fn parse_s3_bucket_and_prefix() {
        let url = Url::parse("s3://my-bucket/a/main.rs").unwrap();
        assert_eq!(url.scheme, "s3");
        assert_eq!(url.host.as_deref(), Some("my-bucket"));
        assert_eq!(url.path, Path::new("a/main.rs"));
        assert_eq!(url.effective_port(), None);
        let bucket_only = Url::parse("s3://my-bucket").unwrap();
        assert_eq!(bucket_only.host.as_deref(), Some("my-bucket"));
        assert!(bucket_only.path.as_os_str().is_empty());
    }

    #[test]
    fn parse_webdav_schemes() {
        let url = Url::parse("webdav://dav.example.com/root").unwrap();
        assert_eq!(url.scheme, "webdav");
        assert_eq!(url.effective_port(), Some(80));
        assert!(!url.uses_tls_by_default());
        let tls = Url::parse("webdavs://dav.example.com/root").unwrap();
        assert_eq!(tls.scheme, "webdavs");
        assert!(tls.uses_tls_by_default());
        assert_eq!(tls.effective_port(), Some(443));
    }

    #[test]
    fn parse_file_urls() {
        let abs = Url::parse("file:///abs/path").unwrap();
        assert_eq!(abs.scheme, "file");
        assert_eq!(abs.host, None);
        assert_eq!(abs.port, None);
        assert_eq!(abs.path, Path::new("/abs/path"));
        let rel = Url::parse("file://./rel/path").unwrap();
        assert_eq!(rel.scheme, "file");
        assert_eq!(rel.path, Path::new("./rel/path"));
    }

    #[test]
    fn parse_bare_paths() {
        let rel = Url::parse("./src").unwrap();
        assert_eq!(rel.scheme, "file");
        assert_eq!(rel.host, None);
        assert_eq!(rel.path, Path::new("./src"));
        assert_eq!(rel.lookup_key(), "file");
        let plain = Url::parse("main.rs").unwrap();
        assert_eq!(plain.scheme, "file");
        assert_eq!(plain.path, Path::new("main.rs"));
    }

    #[test]
    fn parse_rejects_userinfo() {
        assert!(matches!(
            Url::parse("ftp://user:pass@ftp.example.com/pub"),
            Err(UrlError::CredentialsInUrl { .. })
        ));
        assert!(matches!(
            Url::parse("webdav://token@dav.example.com/root"),
            Err(UrlError::CredentialsInUrl { .. })
        ));
    }

    #[test]
    fn parse_rejects_unsupported_scheme() {
        assert_eq!(
            Url::parse("ssh://host/x"),
            Err(UrlError::UnsupportedScheme {
                url: "ssh://host/x".to_owned(),
                scheme: "ssh".to_owned(),
            })
        );
    }

    #[test]
    fn parse_rejects_invalid_port() {
        // Out of range and non-numeric ports both fail; no fallback to the
        // default port, since a typo'd port is not a silent-config excuse.
        assert!(matches!(
            Url::parse("ftp://host:99999/x"),
            Err(UrlError::InvalidPort { .. })
        ));
        assert!(matches!(
            Url::parse("ftp://host:notaport/x"),
            Err(UrlError::InvalidPort { .. })
        ));
    }

    #[test]
    fn parse_rejects_missing_host() {
        assert!(matches!(
            Url::parse("ftp://"),
            Err(UrlError::MissingHost { .. })
        ));
        assert!(matches!(
            Url::parse("ftp:///pub"),
            Err(UrlError::MissingHost { .. })
        ));
        assert!(matches!(
            Url::parse("s3:///key"),
            Err(UrlError::MissingHost { .. })
        ));
    }

    #[test]
    fn parse_ipv6_host() {
        let url = Url::parse("ftp://[fe80::1]:21/pub").unwrap();
        assert_eq!(url.host.as_deref(), Some("fe80::1"));
        assert_eq!(url.port, Some(21));
        assert_eq!(url.path, Path::new("pub"));
        assert!(matches!(
            Url::parse("ftp://[fe80::1/pub"),
            Err(UrlError::InvalidHost { .. })
        ));
    }

    #[test]
    fn parse_rejects_host_in_file_url() {
        // `file://host/path` is a mistake, not a relative path named
        // "host/path"; refuse it instead of guessing.
        assert_eq!(
            Url::parse("file://example.com/pub"),
            Err(UrlError::InvalidHost {
                url: "file://example.com/pub".to_owned(),
                host: "example.com".to_owned(),
            })
        );
    }

    #[test]
    fn from_path_keeps_windows_drive_path() {
        // `from_path` copies the path verbatim; on Windows this is an
        // absolute drive-letter path, elsewhere an opaque relative path.
        let url = Url::from_path(r"C:\src\main.rs");
        assert_eq!(url.scheme, "file");
        assert_eq!(url.host, None);
        assert_eq!(url.port, None);
        assert_eq!(url.path, PathBuf::from(r"C:\src\main.rs"));
    }

    #[test]
    fn lookup_key_matches_documented_examples() {
        let ftp = Url::parse("ftp://ftp.example.com:21/pub").unwrap();
        assert_eq!(ftp.lookup_key(), "ftp:ftp.example.com:21");
        let ftp_no_port = Url::parse("ftp://ftp.example.com/pub").unwrap();
        assert_eq!(ftp_no_port.lookup_key(), "ftp:ftp.example.com");
        let s3 = Url::parse("s3://my-bucket/a/main.rs").unwrap();
        assert_eq!(s3.lookup_key(), "s3:my-bucket");
    }
}
