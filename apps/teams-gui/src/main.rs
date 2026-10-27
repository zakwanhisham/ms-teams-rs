//! `teams-gui` — the native Teams desktop client.
//!
//! GUI toolkit is an **open decision** (see `TODO.md` §0): recommended `iced` 0.14
//! (Elm/MVU + wgpu), alternatives `egui`, `gtk-rs`/GTK4, `slint`. Before committing,
//! validate that remote video frames can be blitted as GPU textures into the widget
//! tree (Phase 2B requirement for in-app calls).
//!
//! No toolkit dependency is pulled in yet on purpose — adding it would compile the whole
//! graphics stack before the decision is made.

fn main() {
    println!("teams-gui {} — not yet implemented.", env!("CARGO_PKG_VERSION"));
    println!("Next: decide the GUI toolkit (TODO.md §0), then Phase 6 builds the UI here.");
}

#[cfg(test)]
mod tests {
    #[test]
    fn binary_compiles() {
        // Sanity test so `cargo test` covers the app crate too.
        assert_eq!(env!("CARGO_PKG_NAME"), "teams-gui");
    }
}
