# AGENTS.md — cocomo

## Guidelines

- **MUST** follow the [Rust Guidelines](./RUST_GUIDELINES.md)

## Structure

Cargo workspace with two crates (`resolver = "3"`):

| Crate | Type | Entrypoint |
|---|---|---|
| `cocomo-core/` | library | `src/lib.rs` (re-exports dirdiff, fsops, fsitem, textdiff, readdir) |
| `cocomo-tui/` | binary `cocomo` | `src/main.rs` (imports app, dialog, dirview, event, keymap, keystate, pending_op, textview, view) |
| Root | workspace | `Cargo.toml` — members only, no deps |

## CLI

```
cocomo -l <left> -r <right>    # both optional; cwd used as default left if -l omitted
```

## Rust conventions

- `edition = "2024"` — newer Rust features available (e.g. `&` in `if let` chains, RPIT lifetime capture).
- `#![allow(dead_code)]` is set on both crates — removing it will fail. Dead code is intentionally tolerated.
- Imports: group by `std` → external crates → `crate` with blank-line separators (existing style).
- Error handling: `thiserror` derive in core, `color-eyre` in TUI. Prefer `color_eyre::Result<()>` return types.
- Doc comments: use `//!` on modules, `///` on items where useful.

## Testing

- Integration tests in `cocomo-core/tests/` (currently 1 test file)
- `cocomo-tui/tests/` exists but is empty
- All tests are async (`#[tokio::test]`)
