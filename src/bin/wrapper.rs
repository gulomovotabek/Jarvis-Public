use std::time::{Duration, Instant};

const MAX_BACKOFF_SECS: u64 = 64;
const INITIAL_BACKOFF_SECS: u64 = 2;
const RESTART_WINDOW_SECS: u64 = 600; // 10 minutes
const MAX_RESTARTS_IN_WINDOW: usize = 10;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let child_binary = if args.len() > 1 {
        args[1].clone()
    } else {
        // Default: find the main binary next to this wrapper
        let self_path = std::env::current_exe().unwrap_or_default();
        let dir = self_path.parent().unwrap_or(std::path::Path::new("."));
        let main_bin = dir.join("telegram-codex-jarvis");
        if main_bin.exists() {
            main_bin.to_string_lossy().to_string()
        } else {
            let fallback = dir.join("telegram-codex-jarvis");
            fallback.to_string_lossy().to_string()
        }
    };

    eprintln!("[wrapper] starting supervisor for: {child_binary}");

    let mut backoff_secs = INITIAL_BACKOFF_SECS;
    let mut restart_times: Vec<Instant> = Vec::new();

    // Setup signal forwarding
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        eprintln!("[wrapper] received shutdown signal");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    })
    .ok();

    loop {
        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!("[wrapper] shutting down");
            break;
        }

        // Check restart frequency
        let now = Instant::now();
        restart_times.retain(|t| now.duration_since(*t) < Duration::from_secs(RESTART_WINDOW_SECS));
        if restart_times.len() >= MAX_RESTARTS_IN_WINDOW {
            eprintln!(
                "[wrapper] too many restarts ({} in {}s window), stopping",
                restart_times.len(),
                RESTART_WINDOW_SECS
            );
            std::process::exit(1);
        }

        eprintln!("[wrapper] spawning child process...");
        let start = Instant::now();

        let status = match std::process::Command::new(&child_binary).status() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[wrapper] failed to spawn child: {e}");
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                continue;
            }
        };

        let elapsed = start.elapsed();
        restart_times.push(Instant::now());

        if status.success() {
            eprintln!("[wrapper] child exited cleanly, stopping");
            break;
        }

        eprintln!(
            "[wrapper] child exited with status {:?} after {:.1}s",
            status.code(),
            elapsed.as_secs_f64()
        );

        // If it ran for a while, reset backoff
        if elapsed > Duration::from_secs(60) {
            backoff_secs = INITIAL_BACKOFF_SECS;
        }

        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }

        eprintln!("[wrapper] restarting in {backoff_secs}s...");
        std::thread::sleep(Duration::from_secs(backoff_secs));
        backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
    }
}
