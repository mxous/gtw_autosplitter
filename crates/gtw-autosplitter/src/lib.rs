#![no_std]

extern crate alloc;

// asr's "alloc" feature requires the consumer to supply a global allocator;
// the crate itself is no_std and ships none. This mirrors
// LiveSplit/auto-splitter-template's own boilerplate.
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use asr::{
    future::{next_tick, retry},
    game_engine::unity::mono::{Module, Version},
    time::Duration,
    timer::{self, TimerState},
    Process,
};
use gtw_logic::{Snapshot, Splitter};

asr::async_main!(stable);
asr::panic_handler!();

/// Candidate process names. Under Proton the game presents as its Windows
/// executable name; the bare name is a fallback.
const PROCESS_NAMES: &[&str] = &["Get To Work.exe", "Get To Work"];

/// Mirror of GTWSplitterBridge.GtwSplitterState. All fields are static, so
/// the generated `read` takes no instance argument.
// The asr crate re-exports the `MonoClass` derive macro under the name
// `Class` from this module (see game_engine::unity::mono::class::Class in
// the pinned asr git dependency); `mono::MonoClass` is not a valid path.
#[derive(asr::game_engine::unity::mono::Class)]
#[allow(non_snake_case)]
struct GtwSplitterState {
    #[static_field]
    Attached: bool,
    #[static_field]
    ProgressLevel: i32,
    #[static_field]
    MaxProgressLevel: i32,
    #[static_field]
    GamePaused: bool,
    #[static_field]
    GameEnded: bool,
    #[static_field]
    TotalIgt: f32,
    #[static_field]
    Mode: i32,
}

async fn main() {
    asr::set_tick_rate(120.0);
    asr::print_message("GTW autosplitter starting.");

    loop {
        let process = wait_attach_any(PROCESS_NAMES).await;
        process
            .until_closes(async {
                on_attach(&process).await;
            })
            .await;
    }
}

/// `Process::wait_attach` takes a single name, so poll the candidates in turn.
async fn wait_attach_any(names: &[&str]) -> Process {
    retry(|| names.iter().find_map(|name| Process::attach(name))).await
}

async fn on_attach(process: &Process) {
    asr::print_message("Attached to the game process.");

    let module = Module::wait_attach(process, Version::V3).await;
    asr::print_message("Attached to the Mono module.");

    let bridge_image = module.wait_get_image(process, "GTWSplitterBridge").await;
    asr::print_message("Found the GTWSplitterBridge image.");

    let state = GtwSplitterState::bind(process, &module, &bridge_image).await;
    asr::print_message("Bound GtwSplitterState. Running.");

    let mut splitter = Splitter::new();
    timer::pause_game_time();

    loop {
        if let Ok(raw) = state.read(process) {
            // The bridge has not found GTWProgressProvider yet; its fields are
            // stale, so feeding them to the state machine would be wrong.
            if raw.Attached {
                let snapshot = Snapshot {
                    progress_level: raw.ProgressLevel,
                    max_progress_level: raw.MaxProgressLevel,
                    game_paused: raw.GamePaused,
                    game_ended: raw.GameEnded,
                    total_igt: raw.TotalIgt,
                    // Read from the bridge rather than Isto.Core: the game's IGT
                    // is already load-removed, so this is diagnostic only.
                    currently_loading: false,
                };

                let actions = splitter.update(snapshot);

                if actions.reset {
                    asr::print_message("reset");
                    timer::reset();
                }
                if actions.start {
                    asr::print_message("start");
                    timer::start();
                    timer::pause_game_time();
                }
                if actions.split {
                    asr::print_message("split");
                    timer::split();
                }

                if matches!(timer::state(), TimerState::Running | TimerState::Paused) {
                    timer::set_game_time(Duration::seconds_f64(raw.TotalIgt as f64));
                }
            }
        }

        next_tick().await;
    }
}
