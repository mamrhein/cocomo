# Icons

## App icon

`app-icon.svg` is the application icon (512×512 rounded tile). It mirrors
the status-icon language below: two panes with five circle rows — left
newer (red/gray), right newer (gray/red), same (rings), left orphan (blue,
right side empty) and right orphan (left side empty, blue) — bridged by
two translucent triple-chevron figures: the upper points right, the lower
points left; the heads partially overlap (darker behind lighter) and the
lightest head leads each figure. `app-icon-256.png` is a 256×256 raster
derived from the SVG for packaging, along with `app-icon.icns` (macOS,
types 16 px through 1024 px @2x) and `app-icon.ico` (Windows, PNG entries
16/32/48/64/128/256); only the SVG is embedded into the binary.

## Status icons

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
