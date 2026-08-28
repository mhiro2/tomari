//! Whether the tidied part of the menu bar is showing, and when it should
//! collapse again. No AppKit here: this is the decision layer, and it is what
//! the unit tests below exercise.

/// Whether the items left of Tomari's divider are on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// The divider is stretched, pushing them off the edge of the screen.
    Collapsed,
    /// The divider is back to its normal width and everything is visible.
    Expanded,
}

/// The live expand/collapse state plus the auto-collapse deadline.
///
/// Never persisted: like keep-awake, a launch always starts collapsed. The
/// exception is the launch that first switches the feature on, which the caller
/// starts expanded so the ⌘-drag walkthrough has something to point at.
#[derive(Debug)]
pub struct MenuBarState {
    visibility: Visibility,
    /// How long an expand lasts before collapsing itself, or `None` when the
    /// user has turned the timer off (the default).
    auto_collapse_ms: Option<u64>,
    /// When the current expand runs out, in `AppState::now_ms` terms.
    deadline_ms: Option<u64>,
    /// Bumped by every state change. A timer carries the generation it was
    /// armed for and gives up if it no longer matches, so a collapse scheduled
    /// by an earlier expand cannot fire on a later one — the same
    /// last-writer-wins trick `keepawake` uses across its auth dialog.
    generation: u64,
}

impl MenuBarState {
    pub fn new(auto_collapse_secs: u32) -> Self {
        Self {
            visibility: Visibility::Collapsed,
            auto_collapse_ms: auto_collapse_ms(auto_collapse_secs),
            deadline_ms: None,
            generation: 0,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.visibility == Visibility::Collapsed
    }

    /// The deadline armed, with the generation to hand back to
    /// [`Self::auto_collapse_elapsed`]. `None` when nothing is pending —
    /// collapsed, or expanded with the timer switched off. Production code
    /// uses [`Self::timer_request`], which also carries a clear's generation.
    #[cfg(test)]
    pub fn pending_collapse(&self) -> Option<(u64, u64)> {
        self.deadline_ms.map(|at| (at, self.generation))
    }

    /// What the auto-collapse timer should hold after this state change: the
    /// generation the change produced, and the deadline to fire at (`None` to
    /// clear). Unlike [`Self::pending_collapse`] a clear carries the generation
    /// too, so the timer can order it against an arm that raced it.
    pub fn timer_request(&self) -> (u64, Option<u64>) {
        (self.generation, self.deadline_ms)
    }

    pub fn expand(&mut self, now_ms: u64) {
        self.visibility = Visibility::Expanded;
        self.generation += 1;
        self.deadline_ms = self.auto_collapse_ms.map(|ms| now_ms + ms);
    }

    pub fn collapse(&mut self) {
        self.visibility = Visibility::Collapsed;
        self.generation += 1;
        self.deadline_ms = None;
    }

    pub fn set_collapsed(&mut self, collapsed: bool, now_ms: u64) {
        if collapsed {
            self.collapse();
        } else {
            self.expand(now_ms);
        }
    }

    pub fn toggle(&mut self, now_ms: u64) {
        self.set_collapsed(!self.is_collapsed(), now_ms);
    }

    /// Apply a change to the auto-collapse preference. An expand already in
    /// flight has its deadline redrawn from `now_ms`, so shortening the timer
    /// does not retroactively expire the current expand, and lengthening it
    /// does not leave the old, shorter deadline armed.
    pub fn set_auto_collapse_secs(&mut self, secs: u32, now_ms: u64) {
        let next = auto_collapse_ms(secs);
        if next == self.auto_collapse_ms {
            return;
        }
        self.auto_collapse_ms = next;
        if self.visibility == Visibility::Expanded {
            self.generation += 1;
            self.deadline_ms = next.map(|ms| now_ms + ms);
        }
    }

    /// A timer armed for `generation` has fired. Collapses and returns true
    /// only if that generation is still the current one and we are in fact
    /// expanded; a superseded timer is dropped.
    pub fn auto_collapse_elapsed(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.visibility != Visibility::Expanded {
            return false;
        }
        self.collapse();
        true
    }
}

/// Zero means the user switched the timer off, which is not the same as "expire
/// immediately" — hence `Option` rather than a bare zero.
fn auto_collapse_ms(secs: u32) -> Option<u64> {
    (secs > 0).then(|| u64::from(secs) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_collapsed_with_nothing_pending() {
        let state = MenuBarState::new(0);
        assert!(state.is_collapsed());
        assert_eq!(state.pending_collapse(), None);
    }

    #[test]
    fn toggle_alternates_visibility() {
        let mut state = MenuBarState::new(0);
        state.toggle(1_000);
        assert!(!state.is_collapsed());
        state.toggle(2_000);
        assert!(state.is_collapsed());
    }

    #[test]
    fn expanding_arms_nothing_when_the_timer_is_off() {
        // Zero seconds is the default: an expand stays until the user collapses
        // it, so no deadline is handed out at all.
        let mut state = MenuBarState::new(0);
        state.expand(1_000);
        assert_eq!(state.pending_collapse(), None);
    }

    #[test]
    fn expanding_arms_a_deadline_when_the_timer_is_on() {
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        let (at, _) = state.pending_collapse().expect("deadline armed");
        assert_eq!(at, 16_000);
    }

    #[test]
    fn collapsing_clears_the_deadline() {
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        state.collapse();
        assert_eq!(state.pending_collapse(), None);
    }

    #[test]
    fn the_timer_collapses_the_expand_it_was_armed_for() {
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        let (_, generation) = state.pending_collapse().unwrap();

        assert!(state.auto_collapse_elapsed(generation));
        assert!(state.is_collapsed());
    }

    #[test]
    fn a_timer_from_a_superseded_expand_is_dropped() {
        // Expand, collapse by hand, expand again: the first timer is still out
        // there and must not take down the second expand when it lands.
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        let (_, stale) = state.pending_collapse().unwrap();
        state.collapse();
        state.expand(5_000);

        assert!(!state.auto_collapse_elapsed(stale));
        assert!(
            !state.is_collapsed(),
            "the stale timer must leave the newer expand alone"
        );
    }

    #[test]
    fn a_timer_landing_after_a_manual_collapse_is_dropped() {
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        let (_, generation) = state.pending_collapse().unwrap();
        state.collapse();

        assert!(!state.auto_collapse_elapsed(generation));
    }

    #[test]
    fn changing_the_timer_redraws_the_deadline_of_a_live_expand() {
        // Shortening the timer from 60s to 5s while expanded must measure the
        // new 5s from now, not from the original expand — otherwise the change
        // would collapse the menu bar the instant it is saved.
        let mut state = MenuBarState::new(60);
        state.expand(1_000);
        let (_, before) = state.pending_collapse().unwrap();

        state.set_auto_collapse_secs(5, 30_000);
        let (at, after) = state.pending_collapse().expect("deadline redrawn");

        assert_eq!(at, 35_000);
        assert_ne!(after, before, "the old timer is superseded");
    }

    #[test]
    fn switching_the_timer_off_while_expanded_clears_the_deadline() {
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        state.set_auto_collapse_secs(0, 2_000);
        assert_eq!(state.pending_collapse(), None);
    }

    #[test]
    fn setting_the_same_timer_value_leaves_a_live_expand_alone() {
        // Saving unrelated settings re-applies the same value; that must not
        // keep pushing the deadline out and make the expand effectively
        // permanent for anyone who saves often.
        let mut state = MenuBarState::new(15);
        state.expand(1_000);
        let before = state.pending_collapse().unwrap();

        state.set_auto_collapse_secs(15, 9_000);

        assert_eq!(state.pending_collapse(), Some(before));
    }
}
