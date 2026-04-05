### *Co*mpare, *Co*py & *Mo*ve directories and files

The `cocomo` project is a Rust-based tool for comparing directories and files,
featuring a Terminal User Interface (TUI). It is organized as a Cargo workspace
with two main crates: `cocomo-core` and `cocomo-tui`.

---

#### 1. `cocomo-core` (The Backend Logic)

This crate handles the heavy lifting of filesystem abstraction and comparison
logic. It is designed to be reusable and independent of the user interface.

- **`fsitem.rs`**: Defines `FSItem`, a comprehensive wrapper around filesystem
  entries. It stores metadata (size, modification time, type) and provides
  methods to classify items as directories, files, or symlinks.
- **`readdir.rs`**: Provides asynchronous functions (using `tokio`) to read
  directory contents and populate `FSItem` objects.
- **`fsops.rs`**: Implements file system operations including copy, move,
  delete, and rename. It handles both files and directories recursively. The
  `copy_item` function was updated to correctly handle cases where the
  destination is an existing directory (copies the source item into that
  directory rather than attempting to overwrite it).
- **`dirdiff.rs`**: The heart of the comparison engine.
  - **`DirDiff`**: Performs a side-by-side comparison of two directories. It
    uses a merge-sort-like algorithm to efficiently align items from both sides
    by name.
  - **`DiffItem`**: Represents the result for a single name found in either or
    both directories. It classifies differences as `LeftOnly`, `RightOnly`,
    `Same`, or `Different` (including which side is newer based on modification
    time).

---

#### 2. `cocomo-tui` (The Frontend Application)

This crate provides the interactive terminal interface, built using the
`ratatui` library and `tokio` for async event handling.

- **`main.rs`**: The entry point. It parses command-line arguments (the two
  paths to compare), initializes the terminal, and starts the main application
  loop.
- **`app.rs`**: Manages the global application state. It handles navigation
  between "tabs" (different views), tracks the active view, and processes
  high-level input events. It uses an `AppView` enum to switch between
  directory and file views.
- **`dirview.rs`**: Renders the directory comparison results. It displays a
  list of files and directories from the `DirDiff` result, highlighting
  differences (e.g., items that exist only on one side or are newer).
- **`fileview.rs`**: Provides a side-by-side text comparison view. When a user
  "opens" a file from the directory view, this module reads the content of both
  files and displays them in split panes.
- **`ui.rs`**: Defines the overall layout of the terminal (menu bar, tab bar,
  main content area, and key hint bar) and implements the `Widget` trait for
  the `App` structure.
- **`event.rs`**: Handles terminal events like key presses and window resizing
  in an asynchronous loop.
- **`cmdargs.rs`**: Uses `clap` to handle command-line inputs.

---

#### Summary of Workflow

1. **Initialization**: `cocomo-tui` receives two paths via CLI.
2. **Analysis**: `cocomo-core` scans the paths and generates a `DirDiff` tree.
3. **Interaction**: The user navigates the directory list in the TUI.
4. **Drill-down**: The user can press `Enter` on a file to open a `FileView`,
   which adds a new tab for side-by-side content comparison.
5. **Navigation**: Users can switch between open tabs using `Tab` or close them
   with `x`.
6. **Modification**: Users can copy, move, or delete items using `c`, `m`, or
   `d`. The view automatically refreshes after these operations to display the
   updated filesystem state.

The architecture cleanly separates the data model and comparison logic from the
presentation layer, allowing for potential future expansions (like a web or GUI
frontend) while maintaining a robust core.

---

#### Supported Operations

| Key | Action | Description                                         |
| --- | ------ | --------------------------------------------------- |
| `c` | Copy   | Copies the selected item from one side to the other |
| `m` | Move   | Moves the selected item from one side to the other  |
| `d` | Delete | Deletes the selected item from its current location |
| `r` | Rename | Renames the selected item                           |

When copying or moving items, the operation respects existing directory
structures. For example, if the destination is a directory, the source item is
placed inside that directory using its original name.

The view automatically refreshes after copy, move, delete and rename operations
to reflect the updated filesystem state.
