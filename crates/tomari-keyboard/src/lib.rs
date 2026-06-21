//! `tomari-keyboard` — the keyboard-customization engine.
//!
//! It contains two pure, well-tested pieces:
//! * [`accelerator`] — validate and normalize global-shortcut strings.
//! * [`engine`] — the tap/hold [`ModifierEngine`] that turns raw modifier
//!   activity into [`AppAction`](tomari_core::AppAction)s.
//!
//! Both are free of OS dependencies. The native layer (a CGEventTap requiring
//! the *Input Monitoring* permission) lives in the Tauri app and simply feeds
//! [`KeyEvent`]s into the engine and registers accelerators with Tauri's
//! global-shortcut plugin.

pub mod accelerator;
pub mod engine;
pub mod error;

pub use accelerator::Accelerator;
pub use engine::{HYPER_MODIFIERS, KeyEvent, ModifierEngine};
pub use error::{Error, Result};
