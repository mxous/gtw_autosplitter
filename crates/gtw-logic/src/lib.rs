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

/// Timer operations to perform for a tick. More than one can be set, and they
/// are to be applied in field order: reset, start, then `skips` calls to
/// `skip_split` followed by, if `split`, one real split.
///
/// Splitting is expressed as "skip n, then split" rather than "split n times"
/// because a run can legitimately pass a checkpoint without triggering it (an
/// out-of-bounds skip). The segments whose checkpoints were never triggered are
/// skipped, so the timer records no bogus zero-length segment for them and the
/// whole elapsed span lands on the segment that did end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub reset: bool,
    pub start: bool,
    pub skips: u32,
    pub split: bool,
}

/// IGT is a float accumulated from `Time.deltaTime`, so "went backwards" needs
/// slack to survive the last bits of the sum wobbling.
const IGT_TOLERANCE: f32 = 0.05;

/// Below this IGT, a run that is only now taking its first input has to be a
/// fresh playthrough rather than a save resumed from the main menu: the game's
/// IGT is pause- and load-removed and persists across a save/load, so a resumed
/// run re-enters `PLAYER_FIRST_INPUT` carrying minutes on the clock.
const FRESH_RUN_IGT: f32 = 1.0;

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

    /// High-water mark of the checkpoint index. Bookkeeping only: splits are
    /// driven by [`target_split_index`] against the timer's own index, not by
    /// this. See `update`.
    pub fn highest_progress(&self) -> i32 {
        self.highest_progress
    }

    /// `timer_split_index` is the timer's current split index, or `None` when
    /// no attempt is in progress.
    ///
    /// Splits are decided by comparing that index against the index the game
    /// says the run is at, rather than by reacting to each progress increase.
    /// A single rising edge can be worth more than one split — an out-of-bounds
    /// route can skip a checkpoint entirely, and a reattach can miss a stretch
    /// of them — and reacting per edge silently puts the timer a segment behind
    /// for the rest of the run. Comparing against the real index instead makes
    /// every tick self-correcting.
    pub fn update(&mut self, s: Snapshot, timer_split_index: Option<u32>) -> Actions {
        let mut actions = Actions::default();

        // No transitions can be judged from a single observation.
        let Some(prev) = self.prev.replace(s) else {
            self.highest_progress = s.progress_level;
            return actions;
        };

        // IGT is monotonic within a run, so going backwards means a new run
        // began. Progress can decrease during backtracking within the same run
        // (e.g., loading a checkpoint). Decision of 2026-07-29 by the repo
        // owner: reset keys off IGT alone. An earlier draft also reset when
        // progress decreased, which contradicted
        // `backtracking_does_not_split_or_reset`. Do not reintroduce a
        // progress-based reset without changing that test.
        let igt_went_back = s.total_igt < prev.total_igt - IGT_TOLERANCE;
        if igt_went_back {
            actions.reset = true;
            self.started = false;
            self.highest_progress = s.progress_level;
            return actions;
        }

        if s.progress_level > self.highest_progress {
            self.highest_progress = s.progress_level;
        }

        if !self.started {
            // GTWProgressProvider.Awake sets GamePaused = true; the
            // PLAYER_FIRST_INPUT handler clears it.
            if prev.game_paused && !s.game_paused {
                self.started = true;
                actions.start = true;
                // Closing the game mid-run drops every bit of splitter state
                // while the timer keeps the dead attempt, and no IGT reading
                // survives to notice it going backwards. So a first input that
                // belongs to a fresh playthrough has to clear the timer itself;
                // reset is a no-op when no attempt is running.
                actions.reset = is_fresh_run(&s);
                // Whatever index the timer reports this tick describes the
                // attempt we just reset or adopted, so it says nothing about
                // this run. Align on the next tick.
                return actions;
            }

            // Attached to a run already underway (the splitter restarted, the
            // attempt did not). Adopt it rather than sitting inert for the rest
            // of the run: the timer is on game time, so the clock is already
            // right and only the split index needs catching up. Requiring the
            // timer to be no further along than the game keeps this from
            // adopting a stale attempt left over from a previous run.
            let underway = timer_split_index
                .is_some_and(|index| index <= target_split_index(&s));
            if !underway || is_fresh_run(&s) {
                return actions;
            }
            self.started = true;
        }

        let Some(current) = timer_split_index else {
            return actions;
        };

        // Only ever move forwards. A lower target means backtracking, or a
        // manual split ahead of the game; neither is ours to undo.
        let target = target_split_index(&s);
        if target > current {
            actions.skips = target - current - 1;
            actions.split = true;
        }

        actions
    }
}

/// The split index the timer should be on, given what the game reports.
///
/// Reaching checkpoint N ends segment N-1, so "index == highest checkpoint
/// reached" holds for the whole run, and the end of the game ends the last
/// segment, leaving the index at the segment count.
fn target_split_index(s: &Snapshot) -> u32 {
    let max = s.max_progress_level.max(0);
    let level = if s.game_ended {
        max
    } else {
        // GTWProgressProvider.Events_OnGameEnd sets the level to
        // GetMaxGameProgressLevel(), a sentinel one past the last real
        // checkpoint. Clamp it: only `game_ended` may reach the final segment.
        s.progress_level.min(max - 1)
    };
    level.max(0) as u32
}

fn is_fresh_run(s: &Snapshot) -> bool {
    s.total_igt < FRESH_RUN_IGT && s.progress_level <= 0
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

    const NOTHING: Actions = Actions { reset: false, start: false, skips: 0, split: false };

    /// A splitter wired to a timer that reacts the way LiveSplit does, so the
    /// index the splitter reads back is the one its own actions produced.
    struct Sim {
        splitter: Splitter,
        index: Option<u32>,
    }

    impl Sim {
        fn new() -> Self {
            Self { splitter: Splitter::new(), index: None }
        }

        fn tick(&mut self, s: Snapshot) -> Actions {
            let actions = self.splitter.update(s, self.index);
            if actions.reset {
                self.index = None;
            }
            // The runtime ignores start unless the timer is idle, so a
            // running attempt keeps its index.
            if actions.start && self.index.is_none() {
                self.index = Some(0);
            }
            if let Some(index) = self.index {
                self.index = Some(index + actions.skips + actions.split as u32);
            }
            actions
        }

        /// The game process closing: splitter state dies with it, the timer's
        /// does not.
        fn restart_splitter(&mut self) {
            self.splitter = Splitter::new();
        }

        /// Drives a run from the menu to `progress_level`, the ordinary way.
        fn run_to(&mut self, progress_level: i32) -> Snapshot {
            self.tick(menu());
            let mut cur = menu();
            cur.progress_level = 0;
            cur.game_paused = false;
            self.tick(cur);
            for level in 1..=progress_level {
                cur.progress_level = level;
                cur.total_igt += 30.0;
                self.tick(cur);
            }
            cur
        }
    }

    #[test]
    fn first_tick_never_acts() {
        let mut sim = Sim::new();
        assert_eq!(sim.tick(menu()), NOTHING);
    }

    #[test]
    fn starts_when_game_unpauses() {
        let mut sim = Sim::new();
        sim.tick(menu());
        let mut go = menu();
        go.game_paused = false;
        let a = sim.tick(go);
        assert!(a.start, "first player input must start the timer");
        assert!(!a.split);
        assert!(sim.splitter.started());
    }

    #[test]
    fn does_not_start_twice() {
        let mut sim = Sim::new();
        sim.tick(menu());
        let mut go = menu();
        go.game_paused = false;
        sim.tick(go);
        assert_eq!(sim.tick(go), NOTHING);
    }

    /// The active checkpoint snaps to index 0 before first input, so that
    /// transition must not split. See spec, Splits section.
    #[test]
    fn progress_before_start_does_not_split() {
        let mut sim = Sim::new();
        sim.tick(menu());
        let mut cp0 = menu();
        cp0.progress_level = 0;
        let a = sim.tick(cp0);
        assert!(!a.split, "reaching a checkpoint while paused must not split");
        assert!(!a.start);
    }

    #[test]
    fn splits_on_each_progress_increase_after_start() {
        let mut sim = Sim::new();
        sim.tick(menu());
        let mut cur = menu();
        cur.progress_level = 0;
        sim.tick(cur);
        cur.game_paused = false;
        sim.tick(cur);

        for level in 1..=10 {
            cur.progress_level = level;
            cur.total_igt += 30.0;
            let a = sim.tick(cur);
            assert!(a.split, "reaching checkpoint {level} must split");
            assert_eq!(a.skips, 0, "a checkpoint-by-checkpoint run skips nothing");
            assert_eq!(sim.index, Some(level as u32));
        }
    }

    #[test]
    fn does_not_split_twice_for_same_checkpoint() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(3);
        assert_eq!(sim.index, Some(3));
        cur.total_igt += 1.0;
        assert!(!sim.tick(cur).split, "unchanged progress must not split");
    }

    /// Backtracking must not split, and must not lower the split ceiling.
    #[test]
    fn backtracking_does_not_split_or_reset() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(5);
        assert_eq!(sim.index, Some(5));

        cur.progress_level = 4;
        cur.total_igt += 1.0;
        let a = sim.tick(cur);
        assert!(!a.reset, "IGT still rising means the run is alive");
        assert!(!a.split);

        cur.progress_level = 5;
        cur.total_igt += 1.0;
        let a = sim.tick(cur);
        assert!(!a.split, "re-reaching an already-split checkpoint must not split");
        assert_eq!(sim.index, Some(5));
    }

    /// An out-of-bounds route can reach a checkpoint without triggering the one
    /// before it. The high-water mark then jumps by two, and both segments have
    /// to end: the untriggered one skipped, the one that really ended split.
    #[test]
    fn skipped_checkpoint_advances_two_segments() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(4);

        cur.progress_level = 6;
        cur.total_igt += 30.0;
        let a = sim.tick(cur);
        assert_eq!(a.skips, 1, "the checkpoint that was never triggered is skipped");
        assert!(a.split, "the checkpoint that was reached still splits");
        assert_eq!(sim.index, Some(6), "the timer stays level with the game");

        cur.progress_level = 7;
        cur.total_igt += 30.0;
        let a = sim.tick(cur);
        assert_eq!((a.skips, a.split), (0, true), "and the run continues in step");
        assert_eq!(sim.index, Some(7));
    }

    /// Same mechanism, covering the reattach case: a readiness gap can hide
    /// several checkpoints at once.
    #[test]
    fn catches_up_after_missing_several_checkpoints() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(2);

        cur.progress_level = 8;
        cur.total_igt += 300.0;
        let a = sim.tick(cur);
        assert_eq!((a.skips, a.split), (5, true));
        assert_eq!(sim.index, Some(8));
    }

    /// GTWProgressProvider.Events_OnGameEnd sets the index to
    /// GetMaxGameProgressLevel(), a sentinel past the last real checkpoint.
    /// That jump must not produce an extra split.
    #[test]
    fn max_progress_sentinel_does_not_split() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(10);
        assert_eq!(sim.index, Some(10));

        cur.progress_level = 11;
        cur.total_igt += 1.0;
        let a = sim.tick(cur);
        assert!(!a.split, "the max-progress sentinel is not a checkpoint");
        assert_eq!(sim.index, Some(10));
    }

    #[test]
    fn game_ended_produces_the_final_split() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(10);

        cur.game_ended = true;
        cur.progress_level = 11;
        cur.total_igt += 5.0;
        let a = sim.tick(cur);
        assert!(a.split, "EndOfGame must produce the final split");
        assert_eq!(a.skips, 0);
        assert!(!a.reset);
        assert_eq!(sim.index, Some(11), "eleven segments, all of them ended");
    }

    #[test]
    fn game_ended_splits_only_once() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(10);
        cur.game_ended = true;
        cur.total_igt += 5.0;
        assert!(sim.tick(cur).split);
        cur.total_igt += 1.0;
        assert!(!sim.tick(cur).split);
    }

    #[test]
    fn igt_going_backwards_resets() {
        let mut sim = Sim::new();
        sim.run_to(4);

        let mut fresh = menu();
        fresh.game_paused = false;
        let a = sim.tick(fresh);
        assert!(a.reset, "a fresh run must reset the timer");
        assert!(!sim.splitter.started());
        assert_eq!(sim.splitter.highest_progress(), -1);
        assert_eq!(sim.index, None);
    }

    #[test]
    fn can_start_a_second_run_after_reset() {
        let mut sim = Sim::new();
        sim.run_to(6);

        assert!(sim.tick(menu()).reset);

        let mut go = menu();
        go.game_paused = false;
        assert!(sim.tick(go).start, "the next run must be able to start");
    }

    /// Closing the game mid-run and starting a fresh one: the splitter loses
    /// every reading it had, so no IGT comparison can notice the new run. The
    /// first input has to clear the attempt the dead run left behind.
    #[test]
    fn fresh_run_after_process_restart_resets_the_stale_attempt() {
        let mut sim = Sim::new();
        sim.run_to(4);
        assert_eq!(sim.index, Some(4));

        sim.restart_splitter();

        sim.tick(menu());
        let mut go = menu();
        go.progress_level = 0;
        go.game_paused = false;
        let a = sim.tick(go);
        assert!(a.reset, "the fresh run must clear the dead run's attempt");
        assert!(a.start);
        assert_eq!(sim.index, Some(0), "and start from the first segment");

        go.progress_level = 1;
        go.total_igt = 30.0;
        let a = sim.tick(go);
        assert_eq!((a.skips, a.split), (0, true));
        assert_eq!(sim.index, Some(1));
    }

    /// Same restart, but the run is resumed from a save rather than restarted.
    /// Its IGT carries over, so the attempt is still the right one and must
    /// survive.
    #[test]
    fn resumed_run_after_process_restart_keeps_the_attempt() {
        let mut sim = Sim::new();
        sim.run_to(4);

        sim.restart_splitter();

        let mut paused = menu();
        paused.progress_level = 4;
        paused.total_igt = 120.0;
        sim.tick(paused);

        let mut go = paused;
        go.game_paused = false;
        let a = sim.tick(go);
        assert!(!a.reset, "a resumed run is not a new attempt");
        assert!(a.start, "start is a no-op on a running timer, and needed if not");
        assert_eq!(sim.index, Some(4), "the timer stays where the run left it");

        go.progress_level = 5;
        go.total_igt = 150.0;
        let a = sim.tick(go);
        assert_eq!((a.skips, a.split), (0, true));
        assert_eq!(sim.index, Some(5));
    }

    /// Attaching to a run already in progress, with no start edge to see: the
    /// attempt is adopted and the index caught up, rather than the splitter
    /// sitting inert for the rest of the run.
    #[test]
    fn adopts_a_run_already_underway() {
        let mut sim = Sim::new();
        sim.run_to(3);

        sim.restart_splitter();

        let mut cur = menu();
        cur.game_paused = false;
        cur.progress_level = 5;
        cur.total_igt = 200.0;
        sim.tick(cur);

        cur.total_igt = 201.0;
        let a = sim.tick(cur);
        assert!(!a.reset);
        assert!(!a.start, "the attempt is already running");
        assert_eq!((a.skips, a.split), (1, true), "catch the index up to checkpoint 5");
        assert_eq!(sim.index, Some(5));
        assert!(sim.splitter.started());
    }

    /// The same adoption must not happen against an attempt that cannot belong
    /// to this run: a fresh playthrough with a stale attempt sitting well ahead
    /// of it waits for the start edge, which resets.
    #[test]
    fn does_not_adopt_a_stale_attempt_for_a_fresh_run() {
        let mut sim = Sim::new();
        sim.run_to(7);

        sim.restart_splitter();

        let mut cur = menu();
        cur.progress_level = 0;
        sim.tick(cur);
        assert_eq!(sim.tick(cur), NOTHING, "nothing to adopt here");
        assert_eq!(sim.index, Some(7), "the stale attempt is left untouched");

        cur.game_paused = false;
        assert!(sim.tick(cur).reset, "the start edge clears it");
    }

    /// Loading flags are read for diagnostics only and must not gate splits,
    /// because the game's IGT is already load-removed.
    #[test]
    fn loading_flag_does_not_suppress_splits() {
        let mut sim = Sim::new();
        let mut cur = sim.run_to(1);
        cur.currently_loading = true;
        cur.progress_level = 2;
        cur.total_igt += 30.0;
        assert!(sim.tick(cur).split);
    }
}
