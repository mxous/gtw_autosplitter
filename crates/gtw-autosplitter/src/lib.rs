#![no_std]

extern crate alloc;

// asr's "alloc" feature requires the consumer to supply a global allocator;
// the crate itself is no_std and ships none. This mirrors
// LiveSplit/auto-splitter-template's own boilerplate.
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

use asr::{
    future::{next_tick, retry},
    game_engine::unity::mono::{Image, Module, UnityPointer, Version},
    time::Duration,
    timer::{self, TimerState},
    Address, Process,
};
use gtw_logic::{Snapshot, Splitter};

asr::async_main!(stable);
asr::panic_handler!();

/// Candidate process names. Under Proton the game presents as its Windows
/// executable name; the bare name is a fallback.
const PROCESS_NAMES: &[&str] = &["Get To Work.exe", "Get To Work"];

/// Reference path from a static field to the live `GTWProgressProvider`.
///
/// `GTWProgressProvider` is a scene `MonoBehaviour` with no static accessor of
/// its own, and it lives in the additively-loaded `PlayerEssentials` scene
/// rather than the active one, so asr's `SceneManager` root lookup cannot see
/// it either. A reachability scan of every static field in Assembly-CSharp,
/// Isto.Core, Plugins and Zenject (run mid-run on 2026-08-08) found exactly one
/// path, this one. `PlayerController.CurrentPlayer` and
/// `_playerWearableController` are both assigned in `PlayerController.Awake`;
/// `GTWGameStats._gameProgress` is assigned by Zenject.
///
/// `_gameProgress` is declared as `IGameProgressProvider`, which has no fields.
/// That is fine: `UnityPointer` resolves each hop's class from the object's own
/// vtable at runtime, so the final hop resolves against the concrete
/// `GTWProgressProvider`.
const TO_PROVIDER: [&str; 4] = [
    "<CurrentPlayer>k__BackingField",
    "_playerWearableController",
    "_gtwGameStats",
    "_gameProgress",
];

/// Field offsets inside a Mono `Dictionary<TKey, TValue>` on 64-bit.
///
/// These cannot be resolved from metadata the way ordinary fields are: field
/// offsets of a generic *definition* are not the offsets of an instantiation,
/// and asr does not expose a way to build a `Class` from a live object's
/// vtable. They were instead **measured** by dumping live dictionaries out of
/// the running game on 2026-08-08:
///
/// ```text
/// 0x00 vtable    0x08 sync block
/// 0x10 buckets   0x18 entries   0x20..0x38 comparer/keys/values
/// 0x40 count     0x44 freeList  0x48 freeCount   0x4C version
/// ```
///
/// Note that `count` does NOT follow `entries`: Mono's auto layout emits every
/// reference field before the value-type ones, so declaration order is not
/// memory order. Assuming otherwise (count @0x20) is what made the splitter
/// read 0 for `FilteredLevels.Count` and never arm itself. Both live
/// instantiations, `<int, float>` and `<int, LevelCheckpointData>`, agreed on
/// these offsets.
const DICT_ENTRIES: u64 = 0x18;
const DICT_COUNT: u64 = 0x40;

/// Mono array layout on 64-bit: object header, bounds pointer, then length.
const ARRAY_LEN: u64 = 0x18;
const ARRAY_DATA: u64 = 0x20;

/// `Dictionary<int, float>.Entry` is `{ int hashCode; int next; int key; float
/// value }`, 16 bytes. Confirmed against a live `GameTime`, whose first entry
/// read `hash=0x7FFFFFFF next=-1 key=-1 value=0.0` — the game's `GameTime[-1]`.
///
/// The stride is per-instantiation (the `<int, LevelCheckpointData>` dictionary
/// uses 24-byte entries), so this constant is only valid for `GameTime`.
const ENTRY_SIZE: u64 = 16;
const ENTRY_VALUE: u64 = 12;

/// When true, the splitter logs its reading of the provider once a second, and
/// reports how far the pointer path got whenever a read fails.
///
/// The hardcoded dictionary offsets above are the only part of this splitter
/// that metadata cannot verify, so turn this back on whenever they are touched
/// or the game updates: the OBS log is the only debugger available here.
const LOG_READINGS: bool = false;

/// Anything read out of `GTWProgressProvider` for a single tick, before it is
/// judged good enough to hand to the state machine.
struct RawState {
    init: bool,
    has_checkpoint_data: bool,
    best_checkpoint_index: i32,
    game_paused: bool,
    game_ended: bool,
    max_progress_level: i32,
    total_igt: f32,
}

/// The resolved pointer paths. Each is resolved once and then cached by asr.
struct Provider {
    init: UnityPointer<5>,
    checkpoint_data: UnityPointer<5>,
    best_checkpoint_index: UnityPointer<5>,
    game_paused: UnityPointer<5>,
    game_ended: UnityPointer<5>,
    game_time: UnityPointer<5>,
    filtered_levels: UnityPointer<5>,
}

impl Provider {
    fn new() -> Self {
        Self {
            init: field("_init"),
            checkpoint_data: field("_checkpointData"),
            best_checkpoint_index: field("_bestCheckpointIndex"),
            game_paused: field("<GamePaused>k__BackingField"),
            game_ended: field("<GameEnded>k__BackingField"),
            game_time: field("<GameTime>k__BackingField"),
            filtered_levels: field("_filteredLevelsCache"),
        }
    }

    fn read(&self, process: &Process, module: &Module, image: &Image) -> Option<RawState> {
        // GetMaxGameProgressLevel() returns FilteredLevels.Count. The property
        // rebuilds the cache when it is null, which an external reader cannot
        // do, so a null cache is reported as -1 and gates the whole snapshot.
        let max_progress_level =
            match read_pointer(process, module, image, &self.filtered_levels) {
                Some(dict) => process.read::<i32>(dict + DICT_COUNT).ok()?,
                None => -1,
            };

        let total_igt = match read_pointer(process, module, image, &self.game_time) {
            Some(dict) => sum_dictionary(process, module, dict)?,
            None => 0.0,
        };

        Some(RawState {
            init: self.init.deref(process, module, image).ok()?,
            has_checkpoint_data:
                read_pointer(process, module, image, &self.checkpoint_data).is_some(),
            best_checkpoint_index: self
                .best_checkpoint_index
                .deref(process, module, image)
                .ok()?,
            game_paused: self.game_paused.deref(process, module, image).ok()?,
            game_ended: self.game_ended.deref(process, module, image).ok()?,
            max_progress_level,
            total_igt,
        })
    }
}

/// Reports how far along [`TO_PROVIDER`] the pointer path actually gets.
///
/// Without this, a failure to resolve the path and a provider that is merely
/// not ready yet are indistinguishable: both simply produce no output.
struct Hops([UnityPointer<5>; 4]);

impl Hops {
    fn new() -> Self {
        Self([
            UnityPointer::new("PlayerController", 0, &TO_PROVIDER[..1]),
            UnityPointer::new("PlayerController", 0, &TO_PROVIDER[..2]),
            UnityPointer::new("PlayerController", 0, &TO_PROVIDER[..3]),
            UnityPointer::new("PlayerController", 0, &TO_PROVIDER[..4]),
        ])
    }

    fn report(&self, process: &Process, module: &Module, image: &Image) -> alloc::string::String {
        let mut report = alloc::string::String::new();

        for (index, pointer) in self.0.iter().enumerate() {
            let status = match pointer.deref_offsets(process, module, image) {
                Err(_) => "unresolved",
                Ok(address) => match process.read_pointer(address, module.get_pointer_size()) {
                    Err(_) => "unreadable",
                    Ok(value) if value.is_null() => "null",
                    Ok(_) => "ok",
                },
            };

            report.push_str(&alloc::format!("{}={} ", TO_PROVIDER[index], status));

            if status != "ok" {
                break;
            }
        }

        report
    }
}

fn field(name: &'static str) -> UnityPointer<5> {
    UnityPointer::new(
        "PlayerController",
        0,
        &[
            TO_PROVIDER[0],
            TO_PROVIDER[1],
            TO_PROVIDER[2],
            TO_PROVIDER[3],
            name,
        ],
    )
}

/// Reads a reference-typed field, mapping a null reference to `None`.
fn read_pointer(
    process: &Process,
    module: &Module,
    image: &Image,
    pointer: &UnityPointer<5>,
) -> Option<Address> {
    let field_address = pointer.deref_offsets(process, module, image).ok()?;
    let value = process.read_pointer(field_address, module.get_pointer_size()).ok()?;
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

/// Sums the values of a `Dictionary<int, float>`, which is what
/// `GetTotalGameSecondsElapsedInPlaythrough()` does: the game accumulates
/// `Time.deltaTime` into a per-checkpoint bucket, and total IGT is their sum.
///
/// Entries are never removed from `GameTime`, so every slot below `count` is
/// live and the free list can be ignored.
fn sum_dictionary(process: &Process, module: &Module, dict: Address) -> Option<f32> {
    let count = process.read::<i32>(dict + DICT_COUNT).ok()?;
    if count <= 0 {
        return Some(0.0);
    }

    let entries = process
        .read_pointer(dict + DICT_ENTRIES, module.get_pointer_size())
        .ok()?;
    if entries.is_null() {
        return Some(0.0);
    }

    // The array's own length bounds the walk, so a wrong `count` cannot make
    // this read past the end of the allocation.
    let length = process.read::<i32>(entries + ARRAY_LEN).ok()?;
    let live = count.min(length);

    let mut total = 0.0;
    for index in 0..live as u64 {
        let value = process
            .read::<f32>(entries + ARRAY_DATA + index * ENTRY_SIZE + ENTRY_VALUE)
            .ok()?;
        total += value;
    }

    Some(total)
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

    let image = module.wait_get_default_image(process).await;
    asr::print_message("Found the Assembly-CSharp image. Running.");

    let provider = Provider::new();
    let hops = Hops::new();
    let mut splitter = Splitter::new();
    let mut was_attached = false;
    let mut ticks: u32 = 0;
    timer::pause_game_time();

    loop {
        // Every read hangs off PlayerController.CurrentPlayer, which is null
        // outside a run, so failure here is the normal menu state.
        if let Some(raw) = provider.read(process, &module, &image) {
            // Mirrors the old bridge mod's `Attached` flag: until the provider
            // has found its CheckpointManager and built the filtered-level
            // cache, its fields are not yet meaningful.
            let attached = raw.init && raw.has_checkpoint_data && raw.max_progress_level > 0;

            if attached != was_attached {
                was_attached = attached;
                asr::print_message(if attached {
                    "Bound GTWProgressProvider."
                } else {
                    "GTWProgressProvider is not ready."
                });
            }

            if !attached && LOG_READINGS && ticks % 120 == 0 {
                asr::print_message(&alloc::format!(
                    "not ready: init={} checkpointData={} max={}",
                    raw.init,
                    raw.has_checkpoint_data,
                    raw.max_progress_level,
                ));
            }

            if attached {
                if LOG_READINGS && ticks % 120 == 0 {
                    asr::print_message(&alloc::format!(
                        "progress={} max={} paused={} ended={} igt={:.4}",
                        raw.best_checkpoint_index,
                        raw.max_progress_level,
                        raw.game_paused,
                        raw.game_ended,
                        raw.total_igt,
                    ));
                }

                let snapshot = Snapshot {
                    // GetCurrentGameProgressLevel() is `_bestCheckpointIndex`
                    // guarded by the same conditions as `attached` above, so
                    // the raw index is the right value to use here.
                    progress_level: raw.best_checkpoint_index,
                    max_progress_level: raw.max_progress_level,
                    game_paused: raw.game_paused,
                    game_ended: raw.game_ended,
                    total_igt: raw.total_igt,
                    // The game's IGT is already load-removed, so this is
                    // diagnostic only. Ruling of 2026-07-29 by the repo owner.
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
                
                if matches!(timer::state(), TimerState::Running | TimerState::Paused) {
                    timer::set_game_time(Duration::seconds_f64(raw.total_igt as f64));
                }

                if actions.split {
                    asr::print_message("split");
                    timer::split();
                }
            }
        } else {
            if was_attached {
                was_attached = false;
                asr::print_message("Lost the provider (no run in progress).");
            }

            if LOG_READINGS && ticks % 120 == 0 {
                asr::print_message(&alloc::format!(
                    "no read: {}",
                    hops.report(process, &module, &image)
                ));
            }
        }

        ticks = ticks.wrapping_add(1);
        next_tick().await;
    }
}
