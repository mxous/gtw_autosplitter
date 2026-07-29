#![no_std]

/// Everything the autosplitter reads from the game in a single tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub progress_level: i32,
    pub max_progress_level: i32,
    pub game_paused: bool,
    pub game_ended: bool,
    pub total_igt: f32,
    pub currently_loading: bool,
}

/// Timer operations to perform for a tick. More than one can be set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub reset: bool,
    pub start: bool,
    pub split: bool,
}

// Deliberately not Default: a derived Default would disagree with new()
// about the initial value of highest_progress.
#[derive(Debug)]
pub struct Splitter {
    prev: Option<Snapshot>,
    highest_progress: i32,
    started: bool,
}

impl Splitter {
    pub fn new() -> Self {
        Self { prev: None, highest_progress: i32::MIN, started: false }
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn highest_progress(&self) -> i32 {
        self.highest_progress
    }

    pub fn update(&mut self, s: Snapshot) -> Actions {
        let mut actions = Actions::default();

        // No transitions can be judged from a single observation.
        let Some(prev) = self.prev.replace(s) else {
            self.highest_progress = s.progress_level;
            return actions;
        };

        // IGT is monotonic within a run, so going backwards means a new run
        // began. IGT is compared with a tolerance because it is a float
        // accumulated from Time.deltaTime. Progress can decrease during
        // backtracking within the same run (e.g., loading a checkpoint).
        let igt_went_back = s.total_igt < prev.total_igt - 0.05;
        if igt_went_back {
            actions.reset = true;
            self.started = false;
            self.highest_progress = s.progress_level;
            return actions;
        }

        // GTWProgressProvider.Awake sets GamePaused = true; the
        // PLAYER_FIRST_INPUT handler clears it.
        if !self.started && prev.game_paused && !s.game_paused {
            self.started = true;
            actions.start = true;
        }

        // The EndOfGame handler sets the index to GetMaxGameProgressLevel(),
        // a sentinel one past the last real checkpoint. Never split on it.
        let reached_new_checkpoint = s.progress_level > self.highest_progress;
        if reached_new_checkpoint {
            if self.started && s.progress_level < s.max_progress_level {
                actions.split = true;
            }
            self.highest_progress = s.progress_level;
        }

        if self.started && !prev.game_ended && s.game_ended {
            actions.split = true;
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-run state: sitting at the starting checkpoint, first input not yet given.
    fn menu() -> Snapshot {
        Snapshot {
            progress_level: -1,
            max_progress_level: 11,
            game_paused: true,
            game_ended: false,
            total_igt: 0.0,
            currently_loading: false,
        }
    }

    const NOTHING: Actions = Actions { reset: false, start: false, split: false };

    #[test]
    fn first_tick_never_acts() {
        let mut s = Splitter::new();
        assert_eq!(s.update(menu()), NOTHING);
    }

    #[test]
    fn starts_when_game_unpauses() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut go = menu();
        go.game_paused = false;
        let a = s.update(go);
        assert!(a.start, "first player input must start the timer");
        assert!(!a.split);
        assert!(s.started());
    }

    #[test]
    fn does_not_start_twice() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut go = menu();
        go.game_paused = false;
        s.update(go);
        assert_eq!(s.update(go), NOTHING);
    }

    /// The active checkpoint snaps to index 0 before first input, so that
    /// transition must not split. See spec, Splits section.
    #[test]
    fn progress_before_start_does_not_split() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cp0 = menu();
        cp0.progress_level = 0;
        let a = s.update(cp0);
        assert!(!a.split, "reaching a checkpoint while paused must not split");
        assert!(!a.start);
    }

    #[test]
    fn splits_on_each_progress_increase_after_start() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.progress_level = 0;
        s.update(cur);
        cur.game_paused = false;
        s.update(cur);

        for level in 1..=10 {
            cur.progress_level = level;
            let a = s.update(cur);
            assert!(a.split, "reaching checkpoint {level} must split");
        }
    }

    #[test]
    fn does_not_split_twice_for_same_checkpoint() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        s.update(cur);
        cur.progress_level = 3;
        assert!(s.update(cur).split);
        assert!(!s.update(cur).split, "unchanged progress must not split");
    }

    /// Backtracking must not split, and must not lower the split ceiling.
    #[test]
    fn backtracking_does_not_split_or_reset() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        cur.total_igt = 10.0;
        s.update(cur);
        cur.progress_level = 5;
        s.update(cur);

        cur.progress_level = 4;
        cur.total_igt = 11.0;
        let a = s.update(cur);
        assert!(!a.reset, "IGT still rising means the run is alive");
        assert!(!a.split);

        cur.progress_level = 5;
        let a = s.update(cur);
        assert!(!a.split, "re-reaching an already-split checkpoint must not split");
    }

    /// GTWProgressProvider.Events_OnGameEnd sets the index to
    /// GetMaxGameProgressLevel(), a sentinel past the last real checkpoint.
    /// That jump must not produce an extra split.
    #[test]
    fn max_progress_sentinel_does_not_split() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        s.update(cur);
        cur.progress_level = 10;
        assert!(s.update(cur).split);

        cur.progress_level = 11;
        let a = s.update(cur);
        assert!(!a.split, "the max-progress sentinel is not a checkpoint");
    }

    #[test]
    fn game_ended_produces_the_final_split() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        s.update(cur);

        cur.game_ended = true;
        cur.progress_level = 11;
        let a = s.update(cur);
        assert!(a.split, "EndOfGame must produce the final split");
        assert!(!a.reset);
    }

    #[test]
    fn game_ended_splits_only_once() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        s.update(cur);
        cur.game_ended = true;
        assert!(s.update(cur).split);
        assert!(!s.update(cur).split);
    }

    #[test]
    fn igt_going_backwards_resets() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        cur.total_igt = 120.0;
        cur.progress_level = 4;
        s.update(cur);

        cur.total_igt = 0.0;
        cur.progress_level = -1;
        let a = s.update(cur);
        assert!(a.reset, "a fresh run must reset the timer");
        assert!(!s.started());
        assert_eq!(s.highest_progress(), -1);
    }

    #[test]
    fn can_start_a_second_run_after_reset() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        cur.total_igt = 90.0;
        cur.progress_level = 6;
        s.update(cur);

        let fresh = menu();
        assert!(s.update(fresh).reset);

        let mut go = menu();
        go.game_paused = false;
        assert!(s.update(go).start, "the next run must be able to start");
    }

    /// Loading flags are read for diagnostics only and must not gate splits,
    /// because the game's IGT is already load-removed.
    #[test]
    fn loading_flag_does_not_suppress_splits() {
        let mut s = Splitter::new();
        s.update(menu());
        let mut cur = menu();
        cur.game_paused = false;
        s.update(cur);
        cur.currently_loading = true;
        cur.progress_level = 2;
        assert!(s.update(cur).split);
    }
}
