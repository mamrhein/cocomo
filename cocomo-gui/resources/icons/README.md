# Icons

Status icons for the directory comparison. Each icon shows two columns of
circles: left column = left side, right column = right side.

## Color / layer semantics

| Layer    | Shape                        | Color      |
| -------- | ---------------------------- | ---------- |
| `newer`  | filled circle                | `#e5484d`  |
| `older`  | filled circle                | `#9b9b9b`  |
| `orphan` | filled circle                | `#3b82f6`  |
| `same`   | hollow circle (ring)         | `#9b9b9b`  |

## Layered icons

Multi-color icons are split into one SVG per layer, in a subdirectory named
after the icon. Every layer shares the `viewBox="0 0 512 512"` of the original
icon, so layers can be stacked with absolute positioning or an overlay layout;
stacking all layers of an icon reproduces the original exactly. Suggested
bottom-to-top order: `older`, `newer`, `orphan`, `same`.

Example (`icons/left-newer/`):

```rust
div()
    .relative()
    .child(img().src("icons/left-newer/older.svg").absolute())
    .child(img().src("icons/left-newer/newer.svg").absolute()),
```

The original composite SVGs are kept next to the layer directories as a
rendered reference.

## Single-layer icons

`left-orphans.svg`, `right-orphans.svg`, `orphans-only.svg` (orphan layer
only) and `same-only.svg` (same layer only) contain a single color and are
not split.
