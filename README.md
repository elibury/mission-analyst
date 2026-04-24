# mission-analyst

A host CLI with an embedded WASM skill that solves the Warp hiring challenge.

## What this is

Two things. First, a straight answer to the challenge: given a noisy pipe-delimited log file of space missions, return the security code of the longest completed Mars mission. Second, an architectural demo: the hot path runs inside a pre-compiled WebAssembly skill loaded at runtime, with the CLI acting as host — raw-byte IPC through linear memory, deterministic allocator, result buffer freed through the skill's own allocator so layouts match. Same pattern I use in production on a larger project.

## Answer

`XRT-421-ZQP` — a 1,629-day completed Mars mission, crew of 4, departed 2065-06-05.

Cross-verified across three independent implementations (awk, strict Python, Rust/WASM) and three interpretations of "successful" (Completed only, Completed+Partial Success, Completed+crewed). All converge.

## Benchmark

Three implementations against the full 105,032-line log. `--iters=11`, median reported.

| Runner | Median | Relative |
|---|---|---|
| awk (GNU) | ~620 ms | 1.0x |
| wasm skill (wasmtime) | ~42 ms | ~15x faster |
| native Rust | ~33 ms | ~19x faster |

WASM overhead vs native is ~25%, mostly the memory write on each invocation. For a hot path called many times, amortised cost approaches native.

## Layout

```
mission-analyst/
├── core/              shared parsing logic + unit tests
│   └── src/lib.rs
├── host/              native CLI, wasmtime runtime, I/O, benchmark harness
│   └── src/main.rs
├── skill/             pure Rust → wasm32-unknown-unknown (depends on core)
│   └── src/lib.rs
├── build.sh           compile skill, then host
├── mission_skill.wasm produced by build.sh
└── space_missions.log the challenge data
```

## Build

```sh
./build.sh
```

Requires the `wasm32-unknown-unknown` target:

```sh
rustup target add wasm32-unknown-unknown
```

## Test

```sh
cargo test -p mission-core
```

Ten tests cover line parsing, whitespace tolerance, rejection of noise, field count validation, duration parsing edge cases, crew edge cases, filter matching, empty-result handling, tie-breaking policy, and round-trip encoding.

## Run

```sh
# default: WASM skill, Mars + Completed
./target/release/mission-analyst space_missions.log

# compare WASM vs native Rust, n=11 iterations
./target/release/mission-analyst space_missions.log --bench

# arbitrary filter
./target/release/mission-analyst space_missions.log --destination=Jupiter --status=Failed
```

## ABI

The skill exports three C-ABI functions:

```
alloc(size: u32) -> u32            // 8-byte aligned
dealloc(ptr: u32, size: u32)
analyze(log_ptr, log_len, filter_ptr, filter_len) -> u64
```

The host allocates memory inside the skill, writes the log bytes and a pipe-separated filter string (`destination|status`), then calls `analyze`. The return value packs `(ptr, len)` into a `u64`. Result format:

```
code|mission_id|date|duration_days|crew_size
```

Return conventions:

| Packed | Meaning |
|---|---|
| `0` | skill error (panic, bad UTF-8, allocator failure) |
| `(ptr << 32) \| 0` with `ptr != 0` | no match — ptr is a 1-byte placeholder to free |
| `(ptr << 32) \| len` with `len > 0` | success — decode bytes at ptr |

Host is responsible for calling `dealloc` with the returned `ptr` and `len` (or 1 for the no-match sentinel). Since the skill uses its own allocator for the result, layouts match and the dealloc is safe.

## Why a WASM skill for a 100K-row log

Because the point isn't the log file. It's the pattern. For a small input the native path wins — startup cost dominates. For a hot loop that runs repeatedly over larger inputs, a pre-compiled WASM skill at near-native speed with raw-byte IPC is orders of magnitude faster than shelling out to a JSON-wrapped tool call, and the skill stays sandboxed, portable, and loadable without rebuilding the host. That's the architecture.

## Data analysis

Full breakdown of the 105,032 lines:

| Category | Count |
|---|---|
| Blank | 975 |
| Comments (`#`) | 2,019 |
| Headers (`SYSTEM/CONFIG/CHECKSUM/CHECKPOINT`) | 2,038 |
| Clean 8-field data rows | 100,000 |

Of the 100,000 data rows, 975 are Mars + Completed. Top duration is 1,629 days, which appears exactly once — no tiebreaker required.

Data quality notes I verified:

- All 100,000 data rows have exactly 8 pipe-separated fields
- All durations are clean positive integers — no floats, negatives, or `N/A`
- All status values are one of 6 canonical strings after trim — no casing or unicode variants
- All 17 destinations are clean — no padding variants of `Mars`
- All 100,000 security codes conform to `[A-Z]{3}-[0-9]{3}-[A-Z]{3}`
- One trap to note: `VVD-671-HBW` is a completed Mars mission with crew size 0. Probably a robotic probe misclassified, or intentional noise to test whether a naive solution filters it out. Doesn't affect the answer since it's 1,206 days — shorter than the winner.

## Security & robustness

- Skill is loaded into its own sandboxed wasmtime instance per process
- No host functions imported by the skill — it cannot touch the filesystem, network, or host memory outside its linear memory
- `panic = "abort"` in release profile — a bug in the skill aborts the instance cleanly rather than corrupting host state
- Result round-trip decoding is validated by the host; malformed skill output is surfaced as an error

## Future work I'd do on a real project

- Fuel metering in wasmtime (cap skill execution time)
- Memory limits on the skill instance
- `no_std` skill with a custom allocator (would shrink the wasm from ~24KB)
- Streaming mode: stream log bytes into the skill rather than copying the whole file
- A skill registry with capability-based tool contracts
