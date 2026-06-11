//! The bundle identifier of the frontmost application — used by Quick Peek to
//! tell whether the app it summoned is the one currently in front.
//!
//! Backed by `NSWorkspace.frontmostApplication`, which is safe to read from any
//! thread.

use objc2_app_kit::NSWorkspace;

/// The bundle id of the frontmost application (e.g. `"com.apple.Safari"`), or
/// `None` when there is no frontmost app or it has no bundle identifier.
pub fn current_bundle_id() -> Option<String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let bundle = app.bundleIdentifier()?;
    Some(bundle.to_string())
}
