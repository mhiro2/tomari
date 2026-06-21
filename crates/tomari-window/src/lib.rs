//! `tomari-window` — window-management snapping.
//!
//! The verifiable core is [`geometry::compute_frame`], which maps a
//! [`WindowPreset`](tomari_core::WindowPreset) to a concrete frame inside a
//! screen's work area. The [`WindowManager`] trait wraps that with the ability
//! to read the work area and move the focused window; [`macos::AxWindowManager`]
//! implements it through the Accessibility API.

pub mod error;
pub mod geometry;
pub mod manager;

#[cfg(target_os = "macos")]
pub mod macos;

pub use error::{Error, Result};
pub use geometry::{
    MIN_DRAG_SIZE, compute_frame, drag_move_frame, drag_resize_frame, edge_snap_preset,
    frames_match, next_in_cycle, remap_frame, screen_at_cursor,
};
pub use manager::{
    MockWindowHandle, MockWindowManager, WindowHandle, WindowManager, adjacent_work_area,
};

#[cfg(target_os = "macos")]
pub use macos::{AxWindowManager, DragWindow, request_permission, window_at_point};
