//! macOS permission to synthesize keyboard and mouse events.
//!
//! CoreGraphics permits event creation without granting event posting access.
//! The core-graphics crate does not expose these permission checks.

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

pub fn is_allowed() -> bool {
    // SAFETY: the system function takes no arguments and returns a C bool.
    unsafe { CGPreflightPostEventAccess() }
}

pub fn request() {
    // SAFETY: the system function takes no arguments and owns its permission UI.
    unsafe { CGRequestPostEventAccess() };
}
