//! Temporary screen numbers, owned and removed entirely on the main run loop.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSColor, NSFont, NSPanel, NSScreen, NSStatusWindowLevel, NSTextAlignment, NSTextField,
};
use objc2_foundation::{
    NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};

use super::model::*;

#[derive(Default)]
struct Identification {
    panels: Vec<Retained<NSPanel>>,
    timer: Option<Retained<NSTimer>>,
}
thread_local! {
    static IDENTIFICATION: RefCell<Identification> = RefCell::default();
}

fn validate(ids: &[String]) -> Result<()> {
    let mut unique = HashSet::new();
    if ids.is_empty()
        || ids.len() > 128
        || ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128 || !unique.insert(id))
    {
        return Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "invalid display selection",
        ));
    }
    Ok(())
}

pub fn show(ids: &[String]) -> Result<()> {
    validate(ids)?;
    let main = MainThreadMarker::new().ok_or_else(|| {
        DisplayError::new(
            DisplayErrorCode::System,
            "display identification requires the main thread",
        )
    })?;
    let screens: HashMap<_, _> = NSScreen::screens(main)
        .into_iter()
        .filter_map(|screen| super::native::screen_identity(&screen).map(|id| (id, screen)))
        .collect();
    let mut frames = Vec::new();
    for id in ids {
        let display = super::native::resolve(id)?;
        if display.is_in_mirror_set() {
            return Err(DisplayError::new(
                DisplayErrorCode::Mirrored,
                "mirrored displays share a desktop image",
            ));
        }
        let screen = screens.get(id).ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::Disconnected,
                "the display is no longer available",
            )
        })?;
        frames.push(screen.frame());
    }
    clear();
    let panels = frames
        .into_iter()
        .enumerate()
        .map(|(index, frame)| {
            let panel = crate::overlay::passive_panel(main, NSStatusWindowLevel + 2);
            panel.setHasShadow(true);
            panel.setAlphaValue(0.94);
            let bounds = NSRect::new(
                NSPoint::new(
                    frame.origin.x + (frame.size.width - 160.0) / 2.0,
                    frame.origin.y + (frame.size.height - 120.0) / 2.0,
                ),
                NSSize::new(160.0, 120.0),
            );
            panel.setFrame_display(bounds, true);
            let label =
                NSTextField::labelWithString(&NSString::from_str(&(index + 1).to_string()), main);
            label.setFont(Some(&NSFont::systemFontOfSize(64.0)));
            label.setAlignment(NSTextAlignment::Center);
            label.setTextColor(Some(&NSColor::whiteColor()));
            label.setFrame(NSRect::new(
                NSPoint::new(8.0, 20.0),
                NSSize::new(144.0, 80.0),
            ));
            if let Some(content) = panel.contentView() {
                content.setWantsLayer(true);
                if let Some(layer) = content.layer() {
                    layer.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
                    layer.setCornerRadius(12.0);
                }
                content.addSubview(&label);
            }
            panel.orderFrontRegardless();
            panel
        })
        .collect();
    let block = RcBlock::new(|_: NonNull<NSTimer>| clear());
    // The block captures nothing; the timer is installed only on the main loop.
    // Common modes also remove badges while a native menu is being tracked.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(3.0, false, &block) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    IDENTIFICATION.with(|state| {
        *state.borrow_mut() = Identification {
            panels,
            timer: Some(timer),
        }
    });
    Ok(())
}

pub fn clear() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    IDENTIFICATION.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(timer) = state.timer.take() {
            timer.invalidate();
        }
        for panel in state.panels.drain(..) {
            panel.orderOut(None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identification_rejects_unbounded_or_ambiguous_selections() {
        assert!(validate(&["a".into(), "b".into()]).is_ok());
        for ids in [
            vec![],
            vec!["a".into(), "a".into()],
            vec![String::new()],
            vec!["a".repeat(129)],
        ] {
            assert!(validate(&ids).is_err());
        }
        assert!(validate(&(0..129).map(|index| index.to_string()).collect::<Vec<_>>()).is_err());
    }
}
