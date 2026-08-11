//! Parse benchmark: raw parser throughput plus the full session pipeline.
//!
//! Usage:
//!   cargo run --release -p stdf-core --example parse_bench -- <stdf-file> [runs]
//!
//! Prints per-run wall time for (a) `parse_reader` alone (record framing +
//! field decode, no SQLite / accumulator) and (b) `SessionManager::open_stdf`
//! end to end (waits for the Complete event, which fires only after the
//! writer thread has committed and indexed).
//!
//! Session DBs created by (b) are deleted before exit: benches bypass the
//! app's startup cleanup, so anything left behind leaks multi-GB files in the
//! shared temp workspace.

use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc;
use std::time::Instant;

use stdf_core::sessions::{SessionEvent, SessionManager};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: parse_bench <stdf-file> [runs]");
        std::process::exit(2);
    };
    let runs: usize = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let size = std::fs::metadata(&path).expect("stat input").len();
    let mb = size as f64 / 1048576.0;
    println!("file: {path} ({mb:.1} MB), {runs} run(s)");

    // (a) raw parser. Only meaningful for bare .stdf/.std inputs — anything
    // else (archives, compressed streams) would feed container bytes into the
    // record framing and measure garbage.
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".stdf") || lower.ends_with(".std") {
        for run in 0..runs {
            let file = File::open(&path).expect("open input");
            let mut reader = BufReader::with_capacity(4 << 20, file);
            let start = Instant::now();
            let mut records = 0_u64;
            stdf_core::parser::parse_reader(
                &mut reader,
                size,
                |_record| {
                    records += 1;
                    true
                },
                |_, _| {},
            )
            .expect("parse");
            let dt = start.elapsed().as_secs_f64();
            println!(
                "raw   run {}: {dt:.3}s  {records} records  {:.1} MB/s",
                run + 1,
                mb / dt
            );
        }
    }

    // (b) full pipeline: parse → accumulator → SQLite index → Complete.
    for run in 0..runs {
        let manager = SessionManager::default();
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        let session = manager
            .open_stdf(path.clone(), move |event| match event {
                SessionEvent::Complete(_) => {
                    let _ = tx.send(Ok(()));
                }
                SessionEvent::Error(error) => {
                    let _ = tx.send(Err(error.message));
                }
                _ => {}
            })
            .expect("open_stdf");
        rx.recv().expect("event channel").expect("parse failed");
        let dt = start.elapsed().as_secs_f64();
        println!("full  run {}: {dt:.3}s  {:.1} MB/s", run + 1, mb / dt);
        drop(manager);
        let db =
            stdf_core::sessions::temp_workspace_dir().join(format!("{}.db", session.session_id));
        let _ = std::fs::remove_file(db);
    }
}
