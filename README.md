# Get To Work auto splitter

A [LiveSplit One](https://github.com/LiveSplit/LiveSplitOne) auto splitter for the Unity game
**Get To Work**. It starts the timer on your first input, splits on every career checkpoint,
and runs on the game's own in-game time.

The splitter reads the game's `GTWProgressProvider` directly out of the running process.

## Install

1. Download `gtw_autosplitter.wasm` from the [latest release](../../releases/latest).
2. In OBS, open the properties of your **LiveSplit One** source, tick **Use local auto
   splitter**, and select the downloaded file.
3. Load `splits/get-to-work.lss` (or your own 11-segment splits) and make sure your layout
   displays **Game Time**, not Real Time — the splitter drives game time.

The same file should also work in desktop LiveSplit via its Auto Splitting Runtime component,
though that path is untested.

## What it does

| Event in game | What the timer does |
| --- | --- |
| First player input | Starts |
| Reaching a checkpoint | Splits |
| Skipping a checkpoint out of bounds | Skips the untriggered segment and splits the one that ended |
| Finishing the game | Final split |
| Starting a new playthrough | Resets |

Timing is **in-game time**: the value the game itself accumulates per checkpoint, already
load-removed and pause-removed. The splitter sums it and pushes it into LiveSplit's game time
every tick, so pausing, loading and menus cost you nothing.

## Splits

`splits/get-to-work.lss` is an empty 11-segment splits file matching the main game's checkpoints:

| # | Segment | | # | Segment |
| --- | --- | --- | --- | --- |
| 1 | Applying For Jobs | | 7 | Middle Manager Interview |
| 2 | Your First Interview | | 8 | Middle Management |
| 3 | Warehouse Trainee | | 9 | Department Head |
| 4 | Warehouse Worker | | 10 | Vice President |
| 5 | Unpaid Intern | | 11 | CEO |
| 6 | Junior Financial Analyst | | | |

A segment ends when you reach the *next* checkpoint, and the last one ends when the game does.
Any 11-segment layout works; only the count matters.

## Building

```bash
cargo build -p gtw-autosplitter --release --target wasm32-unknown-unknown
cargo test -p gtw-logic
```

The output is `target/wasm32-unknown-unknown/release/gtw_autosplitter.wasm`. The
`wasm32-unknown-unknown` target is pinned in `rust-toolchain.toml`, so a plain `rustup`
toolchain is all you need.

The split logic lives in `crates/gtw-logic`, a `no_std`, dependency-free state machine that can
be tested without the game, a process, or wasm. `crates/gtw-autosplitter` does the process and
Mono attaching and nothing else.

Releases are cut by CI from the workspace version: every push to `main` builds and tests, and
publishes `v<version>` with the `.wasm` attached if that tag does not exist yet. To cut one,
bump `version` in the root `Cargo.toml` and merge.
