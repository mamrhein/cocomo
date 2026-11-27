// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Build script: embeds the Windows application icon and bakes the
//! license name into the binary.
//!
//! GPUI's Windows backend registers its window class with the
//! executable's own icon resource, so `app-icon.ico` must be compiled
//! into the binary at link time. On other platforms there is nothing to
//! do: macOS takes the icon from the `.app` bundle and X11 receives it
//! at runtime through `WindowOptions::icon`.
//!
//! The About dialog names the actual license instead of a vague "cocomo
//! license". The license name is the first line of the repository's
//! `LICENSE.TXT`, so it is read at build time and exposed to the
//! sources as `CARGO_PKG_LICENSE_NAME` (usable via `env!`).

fn main() {
    println!("cargo:rerun-if-changed=../LICENSE.TXT");
    let license = std::fs::read_to_string("../LICENSE.TXT")
        .expect("failed to read the workspace LICENSE.TXT")
        .lines()
        .next()
        .expect("LICENSE.TXT is empty")
        .trim()
        .to_owned();
    println!("cargo:rustc-env=CARGO_PKG_LICENSE_NAME={license}");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("resources/icons/app-icon.ico");
        resource
            .compile()
            .expect("failed to embed the Windows app icon");
    }
}
