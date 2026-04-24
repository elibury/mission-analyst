//! mission-analyst — host CLI.
//!
//! Loads a pre-compiled WASM skill (`mission_skill.wasm`) at runtime and
//! dispatches log analysis to it. Mirrors the architecture of a larger
//! skill runtime I maintain: host owns I/O and args, skill owns the hot
//! path, data crosses the boundary via linear memory, result buffer is
//! freed by the host through the skill's own allocator.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};

use mission_core::{find_longest, Best};
use wasmtime::{Engine, Instance, Memory, Module, Store, TypedFunc};

const DEFAULT_SKILL_NAME: &str = "mission_skill.wasm";

// ─── args ──────────────────────────────────────────────────────────────

struct Args {
    log_path: String,
    skill_path: PathBuf,
    destination: String,
    status: String,
    native: bool,
    benchmark: bool,
    bench_iters: u32,
}

fn print_usage() {
    eprintln!("mission-analyst — host CLI with embedded WASM skill runtime\n");
    eprintln!("Usage:");
    eprintln!("  mission-analyst <log-file> [options]\n");
    eprintln!("Options:");
    eprintln!("  --skill=<path>         path to the WASM skill (default: mission_skill.wasm)");
    eprintln!("  --destination=<name>   default: Mars");
    eprintln!("  --status=<name>        default: Completed");
    eprintln!("  --native               run the pure-Rust path instead of the WASM skill");
    eprintln!("  --bench                run WASM + native side-by-side, N iterations, print stats");
    eprintln!("  --iters=N              iterations for --bench (default: 11)");
}

fn parse_args() -> Args {
    let raw: Vec<String> = env::args().collect();
    if raw.len() < 2 || matches!(raw[1].as_str(), "-h" | "--help") {
        print_usage();
        process::exit(1);
    }

    let mut args = Args {
        log_path: raw[1].clone(),
        skill_path: PathBuf::from(DEFAULT_SKILL_NAME),
        destination: "Mars".into(),
        status: "Completed".into(),
        native: false,
        benchmark: false,
        bench_iters: 11,
    };

    for a in &raw[2..] {
        if let Some(v) = a.strip_prefix("--skill=") {
            args.skill_path = PathBuf::from(v);
        } else if let Some(v) = a.strip_prefix("--destination=") {
            args.destination = v.into();
        } else if let Some(v) = a.strip_prefix("--status=") {
            args.status = v.into();
        } else if let Some(v) = a.strip_prefix("--iters=") {
            match v.parse::<u32>() {
                Ok(n) if n > 0 => args.bench_iters = n,
                _ => { eprintln!("invalid --iters value: {v}"); process::exit(1); }
            }
        } else if a == "--native" {
            args.native = true;
        } else if a == "--bench" {
            args.benchmark = true;
        } else {
            eprintln!("unknown argument: {a}");
            process::exit(1);
        }
    }

    args
}

// ─── WASM skill ────────────────────────────────────────────────────────

struct Skill {
    store: Store<()>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    dealloc: TypedFunc<(u32, u32), ()>,
    analyze: TypedFunc<(u32, u32, u32, u32), u64>,
}

impl Skill {
    fn load(path: &PathBuf) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let engine = Engine::default();
        let module = Module::new(&engine, &bytes).map_err(|e| format!("invalid wasm: {e}"))?;
        let mut store: Store<()> = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| format!("instance: {e}"))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| "skill missing `memory` export".to_string())?;
        let alloc = instance
            .get_typed_func::<u32, u32>(&mut store, "alloc")
            .map_err(|e| format!("alloc: {e}"))?;
        let dealloc = instance
            .get_typed_func::<(u32, u32), ()>(&mut store, "dealloc")
            .map_err(|e| format!("dealloc: {e}"))?;
        let analyze = instance
            .get_typed_func::<(u32, u32, u32, u32), u64>(&mut store, "analyze")
            .map_err(|e| format!("analyze: {e}"))?;

        Ok(Skill { store, memory, alloc, dealloc, analyze })
    }

    fn run(&mut self, log: &[u8], filter: &str) -> Result<Option<Best>, String> {
        // copy log into skill memory
        let log_ptr = self.alloc.call(&mut self.store, log.len() as u32)
            .map_err(|e| format!("alloc log: {e}"))?;
        if log_ptr == 0 { return Err("skill failed to allocate log buffer".into()); }
        self.memory.write(&mut self.store, log_ptr as usize, log)
            .map_err(|e| format!("write log: {e}"))?;

        // copy filter
        let filter_bytes = filter.as_bytes();
        let filter_ptr = self.alloc.call(&mut self.store, filter_bytes.len() as u32)
            .map_err(|e| format!("alloc filter: {e}"))?;
        if filter_ptr == 0 { return Err("skill failed to allocate filter buffer".into()); }
        self.memory.write(&mut self.store, filter_ptr as usize, filter_bytes)
            .map_err(|e| format!("write filter: {e}"))?;

        let packed = self.analyze
            .call(&mut self.store, (log_ptr, log.len() as u32, filter_ptr, filter_bytes.len() as u32))
            .map_err(|e| format!("analyze: {e}"))?;

        // Free the inputs regardless of outcome
        let _ = self.dealloc.call(&mut self.store, (log_ptr, log.len() as u32));
        let _ = self.dealloc.call(&mut self.store, (filter_ptr, filter_bytes.len() as u32));

        if packed == 0 {
            return Err("skill returned error sentinel".into());
        }

        let result_ptr = (packed >> 32) as u32;
        let result_len = (packed & 0xFFFF_FFFF) as u32;

        if result_ptr == 0 {
            return Err("skill returned null ptr with non-zero packed".into());
        }

        // read result
        let mut out = vec![0u8; result_len as usize];
        if result_len > 0 {
            self.memory.read(&mut self.store, result_ptr as usize, &mut out)
                .map_err(|e| format!("read result: {e}"))?;
        }

        // free the result buffer using skill's own allocator (matches alloc path)
        let dealloc_len = if result_len == 0 { 1 } else { result_len };
        let _ = self.dealloc.call(&mut self.store, (result_ptr, dealloc_len));

        if result_len == 0 {
            return Ok(None);
        }

        let result_str = std::str::from_utf8(&out)
            .map_err(|e| format!("result not utf-8: {e}"))?;
        let best = Best::decode(result_str)
            .ok_or_else(|| format!("result had wrong shape: {result_str}"))?;
        Ok(Some(best))
    }
}

// ─── output ────────────────────────────────────────────────────────────

fn print_result(label: &str, best: Option<&Best>, destination: &str, status: &str, elapsed: Duration) {
    println!("{label}");
    println!("  destination : {destination}");
    println!("  status      : {status}");
    match best {
        None => println!("  result      : no match"),
        Some(b) => {
            println!("  ANSWER      : {}", b.code);
            println!("  mission id  : {}", b.id);
            println!("  date        : {}", b.date);
            println!("  duration    : {} days", b.duration);
            println!("  crew        : {}", b.crew);
        }
    }
    println!("  elapsed     : {elapsed:?}");
}

fn stats(samples: &[Duration]) -> (Duration, Duration, Duration, Duration) {
    let mut sorted = samples.to_vec();
    sorted.sort();
    let min = *sorted.first().unwrap();
    let max = *sorted.last().unwrap();
    let median = sorted[sorted.len() / 2];
    let sum: Duration = samples.iter().sum();
    let mean = sum / samples.len() as u32;
    (min, median, mean, max)
}

fn fmt_stats(label: &str, s: (Duration, Duration, Duration, Duration)) {
    let (min, med, mean, max) = s;
    println!("  {label:<8} min {min:>12?}  median {med:>12?}  mean {mean:>12?}  max {max:>12?}");
}

// ─── entry ─────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    let log_bytes = match fs::read(&args.log_path) {
        Ok(b) => b,
        Err(e) => { eprintln!("failed to read {}: {e}", args.log_path); process::exit(1); }
    };
    let log_str = match std::str::from_utf8(&log_bytes) {
        Ok(s) => s,
        Err(e) => { eprintln!("log is not valid utf-8: {e}"); process::exit(1); }
    };
    let filter = format!("{}|{}", args.destination, args.status);

    if args.benchmark {
        // native
        let mut native_times = Vec::with_capacity(args.bench_iters as usize);
        let mut native_result = None;
        for _ in 0..args.bench_iters {
            let t = Instant::now();
            native_result = find_longest(log_str, &args.destination, &args.status);
            native_times.push(t.elapsed());
        }

        // wasm
        let mut wasm_times = Vec::with_capacity(args.bench_iters as usize);
        let mut skill = match Skill::load(&args.skill_path) {
            Ok(s) => s,
            Err(e) => { eprintln!("{e}"); process::exit(1); }
        };
        let mut wasm_result = None;
        for _ in 0..args.bench_iters {
            let t = Instant::now();
            wasm_result = match skill.run(&log_bytes, &filter) {
                Ok(r) => r,
                Err(e) => { eprintln!("{e}"); process::exit(1); }
            };
            wasm_times.push(t.elapsed());
        }

        print_result("— native (pure Rust) —", native_result.as_ref(), &args.destination, &args.status, native_times[0]);
        println!();
        print_result("— wasm skill (wasmtime) —", wasm_result.as_ref(), &args.destination, &args.status, wasm_times[0]);
        println!();
        println!("benchmarks (n={}):", args.bench_iters);
        fmt_stats("native", stats(&native_times));
        fmt_stats("wasm", stats(&wasm_times));
        println!();
        println!("both results match: {}", native_result == wasm_result);
        return;
    }

    if args.native {
        let t = Instant::now();
        let r = find_longest(log_str, &args.destination, &args.status);
        let dt = t.elapsed();
        print_result("— native (pure Rust) —", r.as_ref(), &args.destination, &args.status, dt);
    } else {
        let mut skill = match Skill::load(&args.skill_path) {
            Ok(s) => s,
            Err(e) => { eprintln!("{e}"); process::exit(1); }
        };
        let t = Instant::now();
        let r = match skill.run(&log_bytes, &filter) {
            Ok(r) => r,
            Err(e) => { eprintln!("{e}"); process::exit(1); }
        };
        let dt = t.elapsed();
        print_result("— wasm skill (wasmtime) —", r.as_ref(), &args.destination, &args.status, dt);
    }
}
