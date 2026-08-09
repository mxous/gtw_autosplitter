# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A LiveSplit One auto splitter (wasm) for the Unity game **Get To Work**, consumed by the
LiveSplit One browser source in OBS. It reads the game's `GTWProgressProvider` directly through
`asr`'s Mono API and needs no mod installed in the game — see "Mono binding". An earlier design
used a BepInEx bridge mod to mirror run state into flat `public static` fields; it was deleted on
2026-08-08 (last present at commit `fdce124`) once the direct binding was verified.

## Layout

| Path | Role |
| --- | --- |
| `crates/gtw-logic` | `no_std`, dependency-free split-decision state machine. All unit tests live here. |
| `crates/gtw-autosplitter` | `cdylib` → wasm. Process/Mono attach, field binding, timer calls. No split policy. |
| `.superpowers/sdd/2026-07-29-gtw-autosplitter/` | Plan ledger, task briefs/reports, prior review diffs. `progress.md` records binding decisions and open issues — read it before changing split behaviour. |

The split policy is deliberately isolated in `gtw-logic` so it can be tested without a game,
a process, or wasm. Keep it that way: `gtw-autosplitter/src/lib.rs` should only translate
`Snapshot` in / `Actions` out.

## Commands

```bash
cargo test -p gtw-logic                    # the whole test suite
cargo test -p gtw-logic <test_name>        # single test
cargo build -p gtw-autosplitter --release --target wasm32-unknown-unknown
```

Output wasm: `target/wasm32-unknown-unknown/release/gtw_autosplitter.wasm`. The release profile
is size-tuned (`opt-level="z"`, `lto`, `panic="abort"`); `wasm32-unknown-unknown` is pinned in
`rust-toolchain.toml`.

Manual end-to-end check: build the wasm, point the OBS LiveSplit One source at it with
"Use local auto splitter" enabled (toggle that off/on to reload after a rebuild — OBS caches the
module), start a run, then read the log:

```bash
grep 'Auto Splitter' ~/.config/obs-studio/logs/$(ls -t ~/.config/obs-studio/logs | head -1)
```

Expect: "Attached to the game process." → "Attached to the Mono module." → "Found the
Assembly-CSharp image." → "Bound GTWProgressProvider." on entering a run.

## Game facts these depend on

- `GTWProgressProvider` (Assembly-CSharp, `Isto.GTW`) is the source of everything. It is
  Zenject-injected, so `_gameState` is null between `Awake` and the `[Inject]` call; nothing it
  owns can be trusted until `_init` is set.
- `GetCurrentGameProgressLevel()` returns `_bestCheckpointIndex`, a high-water mark; `-1`
  before init/first checkpoint.
- `GetMaxGameProgressLevel()` is `11` for the main game and doubles as a sentinel: the
  `EndOfGame` handler sets the current level to it. **Never split on `progress_level ==
  max_progress_level`.**
- `GamePaused` starts `true` in `Awake` and is cleared by the `PLAYER_FIRST_INPUT` handler —
  that `true → false` edge is the run start.
- `GetTotalGameSecondsElapsedInPlaythrough()` sums the per-checkpoint `GameTime` dictionary.
  It is already load- and pause-removed, which is why `Snapshot.currently_loading` is hardcoded
  `false` and must not gate splits.
- Measured 2026-07-29 on the main game (`Mode`/`SaveSlot` 0): checkpoints 0..10 are
  Applying For Jobs, Your First Interview, Warehouse Trainee, Warehouse Worker, Unpaid Intern,
  Junior Financial Analyst, Middle Manager Interview, Middle Management, Department Head,
  Vice President, CEO — 1:1 with the `.lss` segments. Checkpoint 0 is reached *before* first
  input, so it produces no split. Total splits = cp1..cp10 + `GameEnded` = 11.

## Split-logic invariants (don't regress these)

- Reset keys off **IGT going backwards only** (tolerance 0.05s), never off progress decreasing.
  Owner ruling 2026-07-29; backtracking within a run is legal. See the comment in
  `crates/gtw-logic/src/lib.rs` and `backtracking_does_not_split_or_reset`.
- First tick never acts (no transition can be inferred from one observation).
- Splits fire on the rising edge of the high-water mark only, and only after `started`.
- `Splitter` intentionally has no `Default` — it would disagree with `new()` about
  `highest_progress`.

## Known open issues

Both are recorded in `.superpowers/.../progress.md` and were carried forward rather than fixed:

1. **Mid-run reattach loses splits.** If readiness flips false→true mid-run and progress advanced
   by more than one checkpoint during the gap, one split fires instead of N. Fix is catch-up
   splitting in `gtw-logic`, bounded by `max_progress_level`.
2. **Phantom first split (timing-dependent).** Checkpoint 0 landing before `PLAYER_FIRST_INPUT`
   is observed, not guaranteed. Cheap hardening: suppress the split when `highest_progress < 0`.

## Mono binding

Verified end to end in OBS on 2026-08-08: the timer started on first input and split on
checkpoints. The binding, measured in-game:

- **Anchor.** `GTWProgressProvider` has no static accessor and lives in the additively-loaded
  `PlayerEssentials` scene while the *active* scene is `Main_Lighting` — so asr's `SceneManager`
  root lookup cannot reach it either (it only walks the active scene and `DontDestroyOnLoad`).
  A reachability scan of every static field in the game assemblies found exactly one path:
  `PlayerController.<CurrentPlayer>k__BackingField` → `_playerWearableController` →
  `_gtwGameStats` → `_gameProgress`. It is walked with `mono::UnityPointer`, which resolves each
  hop's class from the live object's vtable — so `_gameProgress` being declared as the fieldless
  `IGameProgressProvider` does not matter.
- **Readiness.** Snapshots are gated on `_init && _checkpointData != null &&
  FilteredLevels.Count > 0`. The caches are built lazily by the game's own `Update`, so the
  instant `_init` flips is too early to read them. `PlayerController.CurrentPlayer` is null
  outside a run, so read failures are the normal menu state, not errors.
- **IGT.** No scalar exists: `GetTotalGameSecondsElapsedInPlaythrough()` sums the
  `Dictionary<int,float> GameTime`, whose buckets are fed `+= Time.deltaTime` per checkpoint.
  The splitter sums the dictionary's entries directly. `TimeManager._totalGameTime` is the same
  clock offset by a run-start constant (measured 2.954s in one run) and was rejected because the
  offset can only be captured by watching the start transition, which breaks on mid-run attach.

**The Mono `Dictionary` layout is hardcoded** (`buckets` @0x10, `entries` @0x18, `count` @0x40,
16-byte `Entry` with key at +8 and value at +12) because generic *definitions* carry no
instantiation offsets and asr exposes no way to build a `Class` from a live vtable. These were
measured from live dictionaries, not assumed — see the constants' doc comment. **Mono's auto
layout emits all reference fields before value-type ones, so declaration order is not memory
order**; assuming `count` followed `entries` cost a long debugging session. Set `LOG_READINGS` in
`crates/gtw-autosplitter/src/lib.rs` back to `true` whenever those offsets are touched or the
game updates: it prints the reading once a second and reports how far the pointer path got when
a read fails, which is the only way to tell "path broken" from "provider not ready yet".

Diagnosing this class of failure: `asr::print_message` output lands in the OBS log
(`[LiveSplit One][Auto Splitter]`), which is the only real debugger available. Keep any
instrumentation *cheap* — constructing a fresh `UnityPointer` re-walks every class in the image,
and doing that in a loop blew the runtime's 5-second watchdog and trapped the module, which
silently truncated the diagnostics and produced badly misleading output.

### Investigating further

If the binding breaks (game update, renamed fields), the technique that established it was a
throwaway BepInEx probe plugin that walked every static field in the game assemblies searching
for a reference path to the live provider, and dumped scene topology, instance identity and
object layouts to files in the game root. It lived at `tools/GtwProbe/`, deleted 2026-08-08.
Two things it taught: wait for an actual run before scanning (on the title screen nothing is
injected and the scan finds nothing), and settle object-identity questions in C# with
`ReferenceEquals` rather than by inference from wasm.

- Decompiled C#: `<game>/decompiled/` (`Assembly-CSharp/GTWProgressProvider.cs`). Regenerate with
  `DOTNET_ROLL_FORWARD=LatestMajor ilspycmd <dll> -p -o <dir>`.
- The game's own `Get To Work_Data/Managed/CLAUDE.md` documents its Zenject/event-bus
  architecture.
- UnityExplorer's C# console rejects ordinary local declarations (`Type x = null;` → CS1525), so
  prefer a throwaway BepInEx plugin over console scripts for anything non-trivial.

## Git

The repo owner runs all git commands. Do the work, leave it uncommitted, and say what's pending.
