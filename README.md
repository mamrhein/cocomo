### *Co*mpare, *Co*py & *Mo*ve directories and files

The `cocomo` project is a Rust-based tool for comparing, synchronizing and
snapshotting directories and files, either on the local file system or on a
remote file store.

## Endpoints

Commands address their inputs as *endpoints*: a plain file system path or a
URL.

- `/abs/path` or `./rel/path` — a local path; a relative path is resolved
  against the current working directory.
- `file:///abs/path` or `file://./rel/path` — the same location written as a
  `file://` URL.
- `ftp://host[:port]/path` and `ftps://host[:port]/path` — a directory or
  file on a remote FTP server (`ftps` connects with TLS by default).
- `s3://bucket/prefix`, `webdav://host/path` and `webdavs://host/path` —
  reserved URL shapes; the backends are not implemented yet, so these
  currently fail with an error.

`dir compare` and `dir sync` accept mixed pairs such as
`ftp://host/pub/src` vs. `./src`: endpoints that share an identity (same
URL scheme, host and port) are serviced by one provider — one connection
for both sides — while different endpoints each resolve their own
provider. Cross-boundary copies stream the content between the two
providers, and cross-boundary moves run as copy + delete (non-atomic).
`text compare` and `text diff` resolve each side on its own, so a remote
file can be compared with a local one.

## Credentials

URLs never carry credentials: `ftp://user:pass@host/…` is rejected. For
remote endpoints, credentials are taken from a connection profile (selected
with `--profile <ID>`, or auto-matched against the stored profiles by
provider type, host and port), from the environment (`COCOMO_FTP_USER`,
`COCOMO_FTP_PASSWORD`, …), from the macOS keychain, or entered
interactively at a terminal. A non-interactive run without a resolvable
secret fails with exit code 2 rather than connecting anonymously.

## Exit codes

- `0` — success, no differences found
- `1` — differences found (comparison commands only)
- `2` — an error occurred
