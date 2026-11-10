mod alloc;
mod cpu;
mod scan;
mod snapshot;
mod sse;

use bumpalo::Bump;
use serde_json::json;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

fn report(
    mode: &str,
    iterations: usize,
    bytes: u64,
    elapsed: u128,
    before: alloc::Stats,
    after: alloc::Stats,
    summary: serde_json::Value,
) {
    let row = json!({
        "mode": mode, "allocator": alloc::name(), "counted": cfg!(feature = "count-alloc"),
        "iterations": iterations, "bytes": bytes, "elapsed_ns": elapsed,
        "allocated_bytes": after.allocated_bytes-before.allocated_bytes,
        "allocations": after.allocations-before.allocations,
        "live_before": before.live_requested_bytes, "live_after": after.live_requested_bytes,
        "peak_requested_bytes": after.peak_requested_bytes, "result": summary,
        "process_cpu_ns": after.process_cpu_ns-before.process_cpu_ns,
    });
    println!("RUSTGO {row}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: kou-rust-go-probe MODE INPUT [ITERATIONS]".into());
    }
    let mode = &args[1];
    let path = Path::new(&args[2]);
    let iterations: usize = args.get(3).map(|n| n.parse()).transpose()?.unwrap_or(1);
    if iterations == 0 {
        return Err("iterations must be positive".into());
    }
    let bytes = std::fs::metadata(path)?.len();
    match mode.as_str() {
        "sse-check" => {
            let cases: Vec<sse::Case> = serde_json::from_slice(&std::fs::read(path)?)?;
            sse::validate(&cases);
            println!("RUSTGO {}", json!({"mode":mode,"cases":cases.len()}));
        }
        "sse" | "sse-chunked" => {
            let encoded = std::fs::read(path)?;
            let chunked = mode == "sse-chunked";
            let expected = sse::run(&encoded, chunked);
            alloc::reset_peak();
            let before = alloc::stats();
            let start = Instant::now();
            let mut summary = (0, 0, 0);
            for _ in 0..iterations {
                summary = black_box(sse::run(black_box(&encoded), chunked));
            }
            let elapsed = start.elapsed().as_nanos();
            let after = alloc::stats();
            assert_eq!(summary, expected);
            report(
                mode,
                iterations,
                bytes,
                elapsed,
                before,
                after,
                json!({"summary":[summary.0,summary.1,summary.2]}),
            );
        }
        "scan" | "scan-owned" => {
            let expected = scan::run(path, true)?;
            alloc::reset_peak();
            let before = alloc::stats();
            let start = Instant::now();
            let mut last = scan::ZERO_TIME;
            for _ in 0..iterations {
                last = black_box(scan::run(path, mode == "scan")?);
            }
            let elapsed = start.elapsed().as_nanos();
            let after = alloc::stats();
            assert_eq!(last, expected);
            report(
                mode,
                iterations,
                bytes,
                elapsed,
                before,
                after,
                json!({"last_seconds":last.0,"last_nanos":last.1}),
            );
        }
        "clone" | "arena" | "arena-fresh" | "clone-check" => {
            let source: Vec<snapshot::Entry> = serde_json::from_slice(&std::fs::read(path)?)?;
            if mode == "clone-check" {
                snapshot::validate(&source);
                let output = std::env::var("RUSTGO_OUTPUT")?;
                std::fs::write(output, serde_json::to_vec(&source)?)?;
                println!("RUSTGO {}", json!({"mode":mode,"entries":source.len()}));
                return Ok(());
            }
            let summary = snapshot::summary(&source);
            let mut arena = Bump::new();
            alloc::reset_peak();
            let before = alloc::stats();
            let start = Instant::now();
            for _ in 0..iterations {
                if mode == "clone" {
                    let cloned = black_box(source.clone());
                    black_box(&cloned);
                } else if mode == "arena-fresh" {
                    let fresh = Bump::new();
                    let cloned = snapshot::arena_clone(black_box(&source), &fresh);
                    black_box(cloned);
                } else {
                    arena.reset();
                    let cloned = snapshot::arena_clone(black_box(&source), &arena);
                    black_box(cloned);
                }
            }
            let elapsed = start.elapsed().as_nanos();
            let after = alloc::stats();
            report(
                mode,
                iterations,
                bytes,
                elapsed,
                before,
                after,
                json!({"summary":summary,"entries":source.len()}),
            );
            black_box(&source);
            black_box(&arena);
        }
        _ => return Err(format!("unknown mode: {mode}").into()),
    }
    Ok(())
}
