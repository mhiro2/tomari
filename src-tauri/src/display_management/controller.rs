//! One serialized owner for display previews, confirmation, and rollback.

use std::time::{Duration, Instant};

use super::configuration::{self, ConfigurationRequest, DisplaySetting};
use super::model::*;

pub const PREVIEW_DURATION: Duration = Duration::from_secs(20);

struct Pending {
    token: u64,
    display_id: String,
    before: Vec<DisplaySetting>,
    after: Vec<DisplaySetting>,
    // Until the first successful read, Quartz may have adjusted only origins.
    // Once observed, ownership requires the exact complete configuration.
    observed: bool,
    deadline: Instant,
}

impl Pending {
    fn matches_applied_modes(&self, actual: &[DisplaySetting]) -> bool {
        actual.len() == self.after.len()
            && actual.iter().zip(&self.after).all(|(actual, expected)| {
                actual.display_id == expected.display_id && actual.mode_id == expected.mode_id
            })
    }

    fn owns(&self, actual: &[DisplaySetting]) -> bool {
        if self.observed {
            actual == self.after
        } else {
            self.matches_applied_modes(actual)
        }
    }
}

pub struct DisplayController<B> {
    backend: B,
    pending: Option<Pending>,
    notice: Option<DisplayNotice>,
    revision: u64,
}

impl<B: DisplayBackend> DisplayController<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            pending: None,
            notice: None,
            revision: 0,
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn capture_profile(
        &mut self,
        name: String,
        clock: impl Fn() -> Instant + Copy,
    ) -> Result<tomari_core::DisplayProfile> {
        let snapshot = self.snapshot(clock)?;
        if snapshot.pending.is_some() {
            return Err(DisplayError::new(
                DisplayErrorCode::Busy,
                "confirm or revert the preview before saving a profile",
            ));
        }
        super::profiles::capture(&snapshot.displays, name)
    }

    pub fn snapshot(&mut self, clock: impl Fn() -> Instant + Copy) -> Result<DisplaySnapshot> {
        self.expire(clock());
        let displays = self.backend.inventory()?;
        if let Some(pending) = &mut self.pending {
            let current = configuration::capture(&displays);
            // Missing mode metadata is not evidence of an external change.
            // Keep ownership and refuse confirmation until a complete read
            // can establish which configuration is actually active.
            if let Err(error) = &current
                && error.code != DisplayErrorCode::Mirrored
            {
                return Err(error.clone());
            }
            if (self.notice == Some(DisplayNotice::RestoreFailed) || !pending.observed)
                && current
                    .as_ref()
                    .is_ok_and(|current| *current == pending.before)
            {
                self.pending = None;
                self.notice = Some(DisplayNotice::Reverted);
                self.revision += 1;
            } else if let Ok(current) = current
                && pending.owns(&current)
            {
                pending.after = current;
                pending.observed = true;
            } else {
                // A physical disconnect or external change invalidates ownership.
                self.pending = None;
                self.notice = Some(DisplayNotice::DisplayChanged);
                self.revision += 1;
            }
        }
        Ok(DisplaySnapshot {
            displays,
            pending: self.pending.as_ref().map(|pending| PreviewInfo {
                token: pending.token,
                display_id: pending.display_id.clone(),
                remaining_ms: pending
                    .deadline
                    .saturating_duration_since(clock())
                    .as_millis() as u64,
            }),
            notice: self.notice,
            revision: self.revision,
        })
    }

    pub fn preview(
        &mut self,
        request: ConfigurationRequest,
        clock: impl Fn() -> Instant + Copy,
    ) -> Result<DisplaySnapshot> {
        self.expire(clock());
        if self.pending.is_some() {
            return Err(DisplayError::new(
                DisplayErrorCode::Busy,
                "finish the current display preview first",
            ));
        }
        let displays = self.backend.inventory()?;
        let before = configuration::capture(&displays)?;
        let after = configuration::plan(&displays, &request)?;
        if before == after {
            return self.snapshot(clock);
        }
        self.backend.apply_configuration(&after, false)?;
        self.revision += 1;
        self.notice = None;
        self.pending = Some(Pending {
            token: self.revision,
            display_id: request.display_id().to_string(),
            before,
            after,
            observed: false,
            deadline: clock() + PREVIEW_DURATION,
        });
        // Quartz can move adjacent displays to keep the desktop connected.
        // Attribute the actual applied topology, not the requested geometry.
        if let Ok(displays) = self.backend.inventory()
            && let Ok(actual) = configuration::capture(&displays)
            && let Some(pending) = self.pending.as_mut()
        {
            if actual == pending.before {
                self.pending = None;
                return Err(DisplayError::new(
                    DisplayErrorCode::System,
                    "macOS kept the previous display configuration",
                ));
            }
            if !pending.matches_applied_modes(&actual) {
                // Only origin adjustments belong to the completed transaction.
                // A different display set or mode can be a concurrent OS change.
                self.pending = None;
                self.notice = Some(DisplayNotice::DisplayChanged);
                return self.changed_snapshot(clock);
            }
            pending.after = actual;
            pending.observed = true;
        }
        self.changed_snapshot(clock)
    }

    pub fn confirm(
        &mut self,
        token: u64,
        clock: impl Fn() -> Instant + Copy,
    ) -> Result<DisplaySnapshot> {
        // Snapshot also detects a display disappearing or being changed by
        // macOS while the confirmation panel is open.
        self.snapshot(clock)?;
        self.expire(clock());
        let pending = self
            .pending
            .as_ref()
            .filter(|p| p.token == token)
            .ok_or_else(|| {
                DisplayError::new(
                    DisplayErrorCode::PreviewExpired,
                    "the display preview has expired",
                )
            })?;
        if self.notice == Some(DisplayNotice::RestoreFailed) {
            return Err(DisplayError::new(
                DisplayErrorCode::PreviewExpired,
                "restore the expired preview before making further changes",
            ));
        }
        self.backend.apply_configuration(&pending.after, true)?;
        self.pending = None;
        self.notice = None;
        self.revision += 1;
        self.changed_snapshot(clock)
    }

    pub fn revert(
        &mut self,
        token: u64,
        clock: impl Fn() -> Instant + Copy,
    ) -> Result<DisplaySnapshot> {
        if self.pending.as_ref().is_none_or(|p| p.token != token) {
            return Err(DisplayError::new(
                DisplayErrorCode::PreviewExpired,
                "the display preview has expired",
            ));
        }
        self.restore();
        self.changed_snapshot(clock)
    }

    pub fn next_timeout(&self, now: Instant) -> Option<Duration> {
        if self.notice == Some(DisplayNotice::RestoreFailed) {
            return None;
        }
        self.pending
            .as_ref()
            .map(|pending| pending.deadline.saturating_duration_since(now))
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        if self.notice != Some(DisplayNotice::RestoreFailed)
            && self.pending.as_ref().is_some_and(|p| now >= p.deadline)
        {
            self.restore();
            return true;
        }
        false
    }

    pub fn restore(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        self.revision += 1;
        let current = self
            .backend
            .inventory()
            .and_then(|displays| configuration::capture(&displays));
        match current {
            Ok(current) if current == pending.before => {
                self.notice = Some(DisplayNotice::Reverted);
                return;
            }
            Ok(current) if !pending.owns(&current) => {
                self.notice = Some(DisplayNotice::DisplayChanged);
                return;
            }
            Err(_) => {
                self.pending = Some(pending);
                self.notice = Some(DisplayNotice::RestoreFailed);
                return;
            }
            _ => {}
        }
        if self
            .backend
            .apply_configuration(&pending.before, false)
            .is_ok()
            && let Ok(actual) = self
                .backend
                .inventory()
                .and_then(|displays| configuration::capture(&displays))
        {
            if actual == pending.before {
                self.notice = Some(DisplayNotice::Reverted);
                return;
            }
            // Quartz accepted the transaction but adjusted the final layout.
            // Retain the observed result and original restore point for retry.
            pending.after = actual;
            pending.observed = true;
        }
        self.pending = Some(pending);
        self.notice = Some(DisplayNotice::RestoreFailed);
    }

    fn changed_snapshot(&mut self, clock: impl Fn() -> Instant + Copy) -> Result<DisplaySnapshot> {
        self.snapshot(clock)
            .map_err(|error| DisplayError::new(DisplayErrorCode::RefreshRequired, error.message))
    }

    pub fn brightness(
        &mut self,
        display_id: &str,
        value: f64,
        clock: impl Fn() -> Instant + Copy,
    ) -> Result<DisplaySnapshot> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(DisplayError::new(
                DisplayErrorCode::InvalidBrightness,
                "brightness must be between zero and one",
            ));
        }
        if self.pending.is_some() {
            return Err(DisplayError::new(
                DisplayErrorCode::Busy,
                "finish the current display preview first",
            ));
        }
        let displays = self.backend.inventory()?;
        if find_display(&displays, display_id)?.brightness.is_none() {
            return Err(DisplayError::new(
                DisplayErrorCode::BrightnessUnavailable,
                "hardware brightness is unavailable for this display",
            ));
        }
        self.backend.set_brightness(display_id, value)?;
        self.revision += 1;
        self.changed_snapshot(clock)
    }
}

fn find_display<'a>(displays: &'a [DisplayInfo], id: &str) -> Result<&'a DisplayInfo> {
    let mut matches = displays.iter().filter(|display| display.id == id);
    let display = matches.next().ok_or_else(|| {
        DisplayError::new(
            DisplayErrorCode::Disconnected,
            "the display is no longer connected",
        )
    })?;
    if matches.next().is_some() {
        return Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "display identity is ambiguous",
        ));
    }
    Ok(display)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        displays: Vec<DisplayInfo>,
        writes: Vec<(Vec<DisplaySetting>, bool)>,
        fail_write: bool,
        clock: Option<std::sync::Arc<std::sync::Mutex<Instant>>>,
        inventory_delay: Duration,
        apply_delay: Duration,
        inventory_error: bool,
        fail_inventory_after_write: bool,
        ignore_writes: bool,
        after_write: Option<Vec<DisplayInfo>>,
    }
    fn mode(id: i32) -> DisplayMode {
        DisplayMode {
            id,
            width: 1920,
            height: 1080,
            pixel_width: 3840,
            pixel_height: 2160,
            refresh_rate: 60.0,
            hi_dpi: true,
        }
    }
    fn controller() -> DisplayController<Fake> {
        DisplayController::new(Fake {
            displays: vec![DisplayInfo {
                connection_id: 1,
                id: "screen-a".into(),
                name: "Test display".into(),
                built_in: false,
                main: true,
                mirrored: false,
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                current_mode: Some(mode(1)),
                modes: vec![mode(1), mode(2)],
                brightness: Some(0.5),
                software_dimming: 0.0,
            }],
            writes: Vec::new(),
            fail_write: false,
            clock: None,
            inventory_delay: Duration::ZERO,
            apply_delay: Duration::ZERO,
            inventory_error: false,
            fail_inventory_after_write: false,
            ignore_writes: false,
            after_write: None,
        })
    }
    fn request(id: i32) -> ConfigurationRequest {
        ConfigurationRequest::Mode {
            display_id: "screen-a".into(),
            mode_id: id,
        }
    }
    fn settings(id: i32) -> Vec<DisplaySetting> {
        vec![DisplaySetting {
            display_id: "screen-a".into(),
            mode_id: id,
            x: 0,
            y: 0,
        }]
    }
    impl DisplayBackend for Fake {
        fn inventory(&mut self) -> Result<Vec<DisplayInfo>> {
            if self.inventory_error {
                return Err(DisplayError::new(
                    DisplayErrorCode::System,
                    "injected inventory failure",
                ));
            }
            if let Some(clock) = &self.clock {
                *clock.lock().unwrap() += self.inventory_delay;
            }
            Ok(self.displays.clone())
        }
        fn apply_configuration(
            &mut self,
            settings: &[DisplaySetting],
            permanent: bool,
        ) -> Result<()> {
            if self.fail_write {
                return Err(DisplayError::new(
                    DisplayErrorCode::System,
                    "injected failure",
                ));
            }
            if let Some(clock) = &self.clock {
                *clock.lock().unwrap() += self.apply_delay;
            }
            self.writes.push((settings.to_vec(), permanent));
            self.inventory_error = self.fail_inventory_after_write;
            if self.ignore_writes {
                return Ok(());
            }
            for setting in settings {
                if let Some(display) = self
                    .displays
                    .iter_mut()
                    .find(|d| d.id == setting.display_id)
                {
                    display.current_mode = Some(mode(setting.mode_id));
                    display.x = setting.x;
                    display.y = setting.y;
                    display.main = setting.x == 0 && setting.y == 0;
                }
            }
            if let Some(displays) = self.after_write.take() {
                self.displays = displays;
            }
            Ok(())
        }
        fn set_brightness(&mut self, _: &str, value: f64) -> Result<()> {
            self.displays[0].brightness = Some(value);
            Ok(())
        }
    }

    #[test]
    fn post_apply_inventory_cannot_attribute_changed_display_sets_or_modes_to_the_preview() {
        for change in 0..3 {
            let mut c = controller();
            let mut actual = c.backend.displays.clone();
            actual[0].current_mode = Some(mode(2));
            match change {
                0 => actual[0].id = "replacement".into(),
                1 => actual[0].current_mode = Some(mode(3)),
                _ => actual.clear(),
            }
            c.backend.after_write = Some(actual);
            let now = Instant::now();
            let snapshot = c.preview(request(2), || now).unwrap();
            assert!(snapshot.pending.is_none());
            assert_eq!(snapshot.notice, Some(DisplayNotice::DisplayChanged));
            assert!(!c.expire(now + PREVIEW_DURATION));
            assert_eq!(c.backend.writes.len(), 1);
        }
    }

    #[test]
    fn post_apply_origin_adjustment_is_owned_and_can_be_reverted() {
        let mut c = controller();
        let mut actual = c.backend.displays.clone();
        actual[0].current_mode = Some(mode(2));
        actual[0].x = 100;
        c.backend.after_write = Some(actual);
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        let restored = c.revert(token, || now).unwrap();
        assert!(restored.pending.is_none());
        assert_eq!(restored.displays[0].x, 0);
        assert_eq!(restored.displays[0].current_mode.as_ref().unwrap().id, 1);
    }

    #[test]
    fn a_failed_initial_read_preserves_rollback_for_quartz_adjusted_origins() {
        for refresh_before_timeout in [true, false] {
            let mut c = controller();
            let mut actual = c.backend.displays.clone();
            actual[0].current_mode = Some(mode(2));
            actual[0].x = 100;
            c.backend.after_write = Some(actual);
            c.backend.fail_inventory_after_write = true;
            let now = Instant::now();
            assert_eq!(
                c.preview(request(2), || now).unwrap_err().code,
                DisplayErrorCode::RefreshRequired
            );
            assert!(c.has_pending());
            c.backend.inventory_error = false;
            c.backend.fail_inventory_after_write = false;
            if refresh_before_timeout {
                let observed = c.snapshot(|| now).unwrap();
                assert!(observed.pending.is_some());
                assert_eq!(observed.displays[0].x, 100);
            }
            assert!(c.expire(now + PREVIEW_DURATION));
            assert_eq!(c.backend.writes.len(), 2);
            assert!(!c.has_pending());
            assert_eq!(c.notice, Some(DisplayNotice::Reverted));
            assert_eq!(c.backend.displays[0].x, 0);
            assert_eq!(c.backend.displays[0].current_mode.as_ref().unwrap().id, 1);
        }
    }

    #[test]
    fn a_delayed_first_observation_still_rejects_a_different_mode() {
        let mut c = controller();
        c.backend.fail_inventory_after_write = true;
        let now = Instant::now();
        assert!(c.preview(request(2), || now).is_err());
        c.backend.inventory_error = false;
        c.backend.displays[0].current_mode = Some(mode(3));
        assert!(c.expire(now + PREVIEW_DURATION));
        assert!(!c.has_pending());
        assert_eq!(c.notice, Some(DisplayNotice::DisplayChanged));
        assert_eq!(c.backend.writes.len(), 1);
    }

    #[test]
    fn preview_is_temporary_and_watchdog_restores_without_ui() {
        let mut c = controller();
        let now = Instant::now();
        let snapshot = c.preview(request(2), || now).unwrap();
        assert_eq!(snapshot.pending.unwrap().remaining_ms, 20_000);
        assert_eq!(c.backend.writes, [(settings(2), false)]);
        c.expire(now + PREVIEW_DURATION);
        assert_eq!(
            c.backend.writes,
            [(settings(2), false), (settings(1), false)]
        );
        assert!(c.pending.is_none());
        assert_eq!(c.notice, Some(DisplayNotice::Reverted));
    }
    #[test]
    fn confirmation_persists_only_the_live_preview() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.confirm(token, || now).unwrap();
        c.expire(now + PREVIEW_DURATION);
        assert_eq!(
            c.backend.writes,
            [(settings(2), false), (settings(2), true)]
        );
    }
    #[test]
    fn expired_confirmation_never_persists() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        assert_eq!(
            c.confirm(token, || now + PREVIEW_DURATION)
                .unwrap_err()
                .code,
            DisplayErrorCode::PreviewExpired
        );
        assert!(c.backend.writes.iter().all(|(_, permanent)| !permanent));
    }
    #[test]
    fn an_old_confirmation_cannot_confirm_a_new_preview() {
        let mut c = controller();
        let now = Instant::now();
        let first = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.revert(first, || now).unwrap();
        let second = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        assert_ne!(first, second);
        assert_eq!(
            c.confirm(first, || now).unwrap_err().code,
            DisplayErrorCode::PreviewExpired
        );
        assert!(c.pending.is_some());
    }
    #[test]
    fn external_changes_are_not_overwritten_on_timeout() {
        let mut c = controller();
        let now = Instant::now();
        c.preview(request(2), || now).unwrap();
        c.backend.displays[0].current_mode = Some(mode(3));
        c.expire(now + PREVIEW_DURATION);
        assert_eq!(c.backend.writes.len(), 1);
        assert_eq!(c.notice, Some(DisplayNotice::DisplayChanged));
    }
    #[test]
    fn disconnect_invalidates_confirmation() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.displays.clear();
        assert_eq!(
            c.confirm(token, || now).unwrap_err().code,
            DisplayErrorCode::PreviewExpired
        );
        assert_eq!(c.backend.writes.len(), 1);
    }
    #[test]
    fn concurrent_previews_and_brightness_are_rejected() {
        let mut c = controller();
        let now = Instant::now();
        c.preview(request(2), || now).unwrap();
        assert_eq!(
            c.preview(request(1), || now).unwrap_err().code,
            DisplayErrorCode::Busy
        );
        assert_eq!(
            c.brightness("screen-a", 0.2, || now).unwrap_err().code,
            DisplayErrorCode::Busy
        );
    }
    #[test]
    fn unsupported_and_mirrored_modes_never_reach_hardware() {
        let mut c = controller();
        let now = Instant::now();
        assert_eq!(
            c.preview(request(99), || now).unwrap_err().code,
            DisplayErrorCode::ModeUnavailable
        );
        c.backend.displays[0].mirrored = true;
        assert_eq!(
            c.preview(request(2), || now).unwrap_err().code,
            DisplayErrorCode::Mirrored
        );
        assert!(c.backend.writes.is_empty());
    }
    #[test]
    fn failed_apply_does_not_create_a_preview() {
        let mut c = controller();
        c.backend.fail_write = true;
        assert!(c.preview(request(2), Instant::now).is_err());
        assert!(c.pending.is_none());
    }
    #[test]
    fn failed_restore_retains_the_restore_point_for_retry() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.fail_write = true;
        c.revert(token, || now).unwrap();
        assert_eq!(c.notice, Some(DisplayNotice::RestoreFailed));
        assert!(c.pending.is_some());
        c.backend.fail_write = false;
        c.revert(token, || now).unwrap();
        assert_eq!(c.backend.displays[0].current_mode.as_ref().unwrap().id, 1);
    }

    #[test]
    fn temporarily_missing_mode_metadata_does_not_discard_the_restore_point() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.displays[0].current_mode = None;
        assert_eq!(
            c.snapshot(|| now).unwrap_err().code,
            DisplayErrorCode::ModeUnavailable
        );
        assert_eq!(
            c.confirm(token, || now).unwrap_err().code,
            DisplayErrorCode::ModeUnavailable
        );
        assert!(c.has_pending());
        assert_eq!(c.backend.writes, [(settings(2), false)]);

        let expired = now + PREVIEW_DURATION;
        c.expire(expired);
        assert_eq!(c.notice, Some(DisplayNotice::RestoreFailed));
        assert!(c.snapshot(|| expired).is_err());
        assert!(c.has_pending());
        c.backend.displays[0].current_mode = Some(mode(2));
        let snapshot = c.snapshot(|| expired).unwrap();
        assert_eq!(snapshot.notice, Some(DisplayNotice::RestoreFailed));
        assert_eq!(snapshot.pending.unwrap().token, token);
        c.revert(token, || expired).unwrap();
        assert_eq!(
            c.backend.writes,
            [(settings(2), false), (settings(1), false)]
        );
        assert!(!c.has_pending());
    }
    #[test]
    fn brightness_rejects_nonfinite_out_of_range_and_unsupported_values() {
        let mut c = controller();
        let now = Instant::now();
        for value in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            assert_eq!(
                c.brightness("screen-a", value, || now).unwrap_err().code,
                DisplayErrorCode::InvalidBrightness
            );
        }
        c.backend.displays[0].brightness = None;
        assert_eq!(
            c.brightness("screen-a", 0.5, || now).unwrap_err().code,
            DisplayErrorCode::BrightnessUnavailable
        );
        assert_eq!(
            c.brightness("absent", 0.5, || now).unwrap_err().code,
            DisplayErrorCode::Disconnected
        );
    }

    #[test]
    fn brightness_requires_one_unambiguous_target_before_calling_the_backend() {
        let mut c = controller();
        c.backend.displays.push(c.backend.displays[0].clone());
        let before = c.backend.displays.clone();
        assert_eq!(
            c.brightness("screen-a", 0.2, Instant::now)
                .unwrap_err()
                .code,
            DisplayErrorCode::Unavailable
        );
        assert_eq!(c.backend.displays, before);
    }
    #[test]
    fn layout_revert_restores_every_display_origin() {
        let mut c = controller();
        let mut secondary = c.backend.displays[0].clone();
        secondary.id = "screen-b".into();
        secondary.x = -1920;
        secondary.main = false;
        c.backend.displays.push(secondary);
        let before = configuration::capture(&c.backend.displays).unwrap();
        let now = Instant::now();
        let token = c
            .preview(
                ConfigurationRequest::Position {
                    display_id: "screen-a".into(),
                    relative_to: "screen-b".into(),
                    placement: super::configuration::RelativePlacement::Below,
                },
                || now,
            )
            .unwrap()
            .pending
            .unwrap()
            .token;
        assert_ne!(configuration::capture(&c.backend.displays).unwrap(), before);
        c.revert(token, || now).unwrap();
        assert_eq!(configuration::capture(&c.backend.displays).unwrap(), before);
    }

    #[test]
    fn an_external_origin_change_cancels_confirmation_without_overwriting_it() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.displays[0].x = 120;
        assert_eq!(
            c.confirm(token, || now).unwrap_err().code,
            DisplayErrorCode::PreviewExpired
        );
        assert_eq!(c.backend.writes.len(), 1);
    }

    #[test]
    fn failed_restore_is_not_retried_automatically_or_confirmable() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.fail_write = true;
        c.expire(now + PREVIEW_DURATION);
        c.backend.fail_write = false;
        c.expire(now + Duration::from_secs(60));
        assert_eq!(c.backend.writes.len(), 1);
        assert_eq!(
            c.confirm(token, || now + Duration::from_secs(60))
                .unwrap_err()
                .code,
            DisplayErrorCode::PreviewExpired
        );
        c.revert(token, || now + Duration::from_secs(60)).unwrap();
        assert_eq!(c.backend.writes.len(), 2);
    }
    #[test]
    fn preview_deadline_starts_after_apply_and_countdown_accounts_for_slow_reads() {
        use std::sync::{Arc, Mutex};
        let mut c = controller();
        let now = Instant::now();
        let clock = Arc::new(Mutex::new(now));
        c.backend.clock = Some(Arc::clone(&clock));
        c.backend.inventory_delay = Duration::from_secs(2);
        c.backend.apply_delay = Duration::from_secs(3);
        let result = c.preview(request(2), || *clock.lock().unwrap()).unwrap();
        assert_eq!(result.pending.unwrap().remaining_ms, 16_000);
        assert_eq!(
            c.pending.as_ref().unwrap().deadline,
            now + Duration::from_secs(25)
        );
    }

    #[test]
    fn confirmation_rechecks_the_deadline_after_a_slow_inventory_read() {
        use std::sync::{Arc, Mutex};
        let mut c = controller();
        let now = Instant::now();
        let clock = Arc::new(Mutex::new(now));
        c.backend.clock = Some(Arc::clone(&clock));
        let token = c
            .preview(request(2), || *clock.lock().unwrap())
            .unwrap()
            .pending
            .unwrap()
            .token;
        *clock.lock().unwrap() = now + Duration::from_secs(19);
        c.backend.inventory_delay = Duration::from_secs(2);
        assert_eq!(
            c.confirm(token, || *clock.lock().unwrap())
                .unwrap_err()
                .code,
            DisplayErrorCode::PreviewExpired
        );
        assert!(c.backend.writes.iter().all(|(_, permanent)| !permanent));
        assert_eq!(c.backend.displays[0].current_mode.as_ref().unwrap().id, 1);
    }
    #[test]
    fn successful_os_return_without_restoration_keeps_the_restore_point() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.ignore_writes = true;
        let snapshot = c.revert(token, || now).unwrap();
        assert_eq!(snapshot.notice, Some(DisplayNotice::RestoreFailed));
        assert!(snapshot.pending.is_some());
        c.backend.ignore_writes = false;
        c.revert(token, || now).unwrap();
        assert!(c.pending.is_none());
    }

    #[test]
    fn a_read_failure_after_confirmation_does_not_rearm_rollback() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.fail_inventory_after_write = true;
        assert_eq!(
            c.confirm(token, || now).unwrap_err().code,
            DisplayErrorCode::RefreshRequired
        );
        assert!(c.pending.is_none());
        assert!(c.backend.writes.last().unwrap().1);
    }

    #[test]
    fn a_later_read_can_confirm_that_an_uncertain_restore_succeeded() {
        let mut c = controller();
        let now = Instant::now();
        let token = c
            .preview(request(2), || now)
            .unwrap()
            .pending
            .unwrap()
            .token;
        c.backend.fail_inventory_after_write = true;
        assert_eq!(
            c.revert(token, || now).unwrap_err().code,
            DisplayErrorCode::RefreshRequired
        );
        assert!(c.pending.is_some());
        c.backend.inventory_error = false;
        let snapshot = c.snapshot(|| now).unwrap();
        assert_eq!(snapshot.notice, Some(DisplayNotice::Reverted));
        assert!(snapshot.pending.is_none());
    }
    #[test]
    fn a_write_that_did_not_change_the_desktop_never_creates_confirmation() {
        let mut c = controller();
        c.backend.ignore_writes = true;
        assert_eq!(
            c.preview(request(2), Instant::now).unwrap_err().code,
            DisplayErrorCode::System
        );
        assert!(c.pending.is_none());
    }
}
