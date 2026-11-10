// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Build script: embeds the Windows application icon.
//!
//! GPUI's Windows backend registers its window class with the
//! executable's own icon resource, so `app-icon.ico` must be compiled
//! into the binary at link time. On other platforms there is nothing to
//! do: macOS takes the icon from the `.app` bundle and X11 receives it
//! at runtime through `WindowOptions::icon`.

fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("resources/icons/app-icon.ico");
        resource
            .compile()
            .expect("failed to embed the Windows app icon");
    }
}
