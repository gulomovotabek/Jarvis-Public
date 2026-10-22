use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use telegram_codex_jarvis::self_update::{self, SelfUpdatePaths};

const POLL: Duration = Duration::from_secs(1);
const DEFAULT_PROBATION_SECS: u64 = 45;

#[derive(Debug, Deserialize)]
struct DeployPending {
    sha: String,
    files: Vec<PathBuf>,
    #[serde(default)]
    job: Option<String>,
}

#[derive(Debug, PartialEq)]
struct GuardArgs {
    health_url: Option<String>,
    data_dir: PathBuf,
    probation: Duration,
    command: Vec<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<GuardArgs, String> {
    let mut args = args.into_iter();
    let mut parsed = GuardArgs {
        health_url: None,
        data_dir: PathBuf::from("data"),
        probation: Duration::from_secs(DEFAULT_PROBATION_SECS),
        command: Vec::new(),
    };
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--health-url" => parsed.health_url = Some(value("--health-url")?).filter(|u| !u.is_empty()),
            "--data-dir" => parsed.data_dir = PathBuf::from(value("--data-dir")?),
            "--probation-secs" => {
                let secs = value("--probation-secs")?;
                parsed.probation = Duration::from_secs(secs.parse().map_err(|_| format!("bad --probation-secs {secs}"))?);
            }
            "--" => {
                parsed.command = args.collect();
                break;
            }
            _ => {
                parsed.command = std::iter::once(arg).chain(args).collect();
                break;
            }
        }
    }
    if parsed.command.is_empty() {
        return Err("usage: jarvis-guard [--health-url URL] [--data-dir DIR] [--probation-secs N] [--] <binary> [args...]".into());
    }
    Ok(parsed)
}

fn main() {
    let args = parse_args(std::env::args().skip(1)).unwrap_or_else(|e| {
        eprintln!("[guard] {e}");
        std::process::exit(2);
    });
    let paths = SelfUpdatePaths::new(&args.data_dir);
    let pending = read_deploy_pending(&paths);

    let mut child = match Command::new(&args.command[0]).args(&args.command[1..]).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("[guard] failed to spawn {}: {e}", args.command[0]);
            if let Some(pending) = &pending {
                fail_deploy(&paths, pending, &format!("spawn failed: {e}"));
            }
            std::process::exit(1);
        }
    };

    if let Some(pending) = &pending {
        eprintln!("[guard] probation for deploy {} ({}s)", pending.sha, args.probation.as_secs());
        match probation(&mut child, args.health_url.as_deref(), args.probation) {
            Ok(()) => {
                eprintln!("[guard] deploy {} passed probation", pending.sha);
                let _ = paths.write_notice(&format!("Deploy {} passed probation, now live.", pending.sha));
                record_job_state(&paths, pending, &format!("live ({})", pending.sha));
                let _ = std::fs::remove_file(paths.deploy_pending());
            }
            Err(reason) => {
                eprintln!("[guard] deploy {} failed probation: {reason}", pending.sha);
                let _ = child.kill();
                let _ = child.wait();
                fail_deploy(&paths, pending, &reason);
                std::process::exit(1);
            }
        }
    }

    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    std::process::exit(code);
}

fn read_deploy_pending(paths: &SelfUpdatePaths) -> Option<DeployPending> {
    let path = paths.deploy_pending();
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<DeployPending>(&text) {
        Ok(pending) => Some(pending),
        Err(e) => {
            eprintln!("[guard] unreadable {}: {e}", path.display());
            let _ = std::fs::remove_file(&path);
            let _ = paths.write_notice(&format!("Ignored unreadable deploy_pending ({e}); started without probation."));
            None
        }
    }
}

fn probation(child: &mut Child, health_url: Option<&str>, window: Duration) -> Result<(), String> {
    let started = Instant::now();
    let mut healthy = health_url.is_none();
    while started.elapsed() < window {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            return Err(format!("exited during probation ({status})"));
        }
        healthy = healthy || health_url.is_some_and(health_ok);
        std::thread::sleep(POLL);
    }
    if healthy {
        Ok(())
    } else {
        Err("health check never returned 200".into())
    }
}

fn fail_deploy(paths: &SelfUpdatePaths, pending: &DeployPending, reason: &str) {
    let errors = rollback(&pending.files);
    let mut text = format!(
        "Deploy {} failed probation: {reason}\nRolled back {} file(s).",
        pending.sha,
        pending.files.len()
    );
    if !errors.is_empty() {
        text.push_str(&format!("\nRollback errors:\n{}", errors.join("\n")));
    }
    let _ = paths.write_notice(&text);
    record_job_state(paths, pending, &format!("rolled back: {reason}"));
    let _ = std::fs::remove_file(paths.deploy_pending());
}

fn record_job_state(paths: &SelfUpdatePaths, pending: &DeployPending, state: &str) {
    if let Some(job) = pending.job.as_deref().filter(|id| self_update::is_job_id(id)) {
        let _ = paths.write_job_state(job, state);
    }
}

fn prev_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".prev");
    PathBuf::from(name)
}

fn rollback(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .filter_map(|file| {
            let prev = prev_path(file);
            let result = if prev.exists() {
                std::fs::rename(&prev, file)
            } else {
                std::fs::remove_file(file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
            };
            result.err().map(|e| format!("{}: {e}", file.display()))
        })
        .collect()
}

fn health_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let Some(addr) = authority.to_socket_addrs().ok().and_then(|mut addrs| addrs.next()) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let request = format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 64];
    let n = stream.read(&mut head).unwrap_or(0);
    status_is_ok(&head[..n])
}

fn status_is_ok(head: &[u8]) -> bool {
    let line = String::from_utf8_lossy(head);
    line.starts_with("HTTP/1.") && line.split_whitespace().nth(1) == Some("200")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("jarvis-guard-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn serve_once(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{addr}/health")
    }

    #[test]
    fn parse_args_reads_flags_and_command() {
        let args = parse_args(
            ["--health-url", "http://127.0.0.1:8081/health", "--probation-secs", "5", "--", "/opt/jarvis/bin/bot", "--x"]
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.health_url.as_deref(), Some("http://127.0.0.1:8081/health"));
        assert_eq!(args.probation, Duration::from_secs(5));
        assert_eq!(args.data_dir, PathBuf::from("data"));
        assert_eq!(args.command, vec!["/opt/jarvis/bin/bot".to_string(), "--x".to_string()]);
        assert!(parse_args(["--health-url".to_string()]).is_err());
        assert!(parse_args(Vec::<String>::new()).is_err());
    }

    #[test]
    fn rollback_restores_prev_and_removes_files_new_in_deploy() {
        let dir = temp_dir("rollback");
        let existing = dir.join("bot");
        let added = dir.join("new_prompt.txt");
        std::fs::write(&existing, "new").unwrap();
        std::fs::write(prev_path(&existing), "old").unwrap();
        std::fs::write(&added, "added").unwrap();
        let missing = dir.join("never_installed");
        assert!(rollback(&[existing.clone(), added.clone(), missing]).is_empty());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "old");
        assert!(!prev_path(&existing).exists());
        assert!(!added.exists());
    }

    #[test]
    fn health_check_requires_http_200() {
        assert!(health_ok(&serve_once("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")));
        assert!(!health_ok(&serve_once("HTTP/1.1 503 Service Unavailable\r\n\r\n")));
        assert!(!health_ok("http://127.0.0.1:1/health"));
        assert!(!health_ok("https://example.com/health"));
    }

    #[test]
    fn probation_fails_when_child_exits_and_passes_when_it_stays_up() {
        let mut quick = Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap();
        assert!(probation(&mut quick, None, Duration::from_secs(3)).unwrap_err().contains("exited during probation"));

        let mut steady = Command::new("sleep").arg("5").spawn().unwrap();
        assert_eq!(probation(&mut steady, None, Duration::from_secs(2)), Ok(()));
        let _ = steady.kill();
        let _ = steady.wait();

        let mut unhealthy = Command::new("sleep").arg("5").spawn().unwrap();
        assert_eq!(
            probation(&mut unhealthy, Some("http://127.0.0.1:1/health"), Duration::from_secs(1)),
            Err("health check never returned 200".to_string())
        );
        let _ = unhealthy.kill();
        let _ = unhealthy.wait();
    }

    #[test]
    fn unreadable_pending_is_dropped_with_notice() {
        let paths = SelfUpdatePaths::new(temp_dir("pending"));
        std::fs::write(paths.deploy_pending(), "{not json").unwrap();
        assert!(read_deploy_pending(&paths).is_none());
        assert!(!paths.deploy_pending().exists());
        assert_eq!(paths.take_notices().len(), 1);

        std::fs::write(paths.deploy_pending(), r#"{"sha":"abc","branch":"jarvis/x","files":["/opt/jarvis/bin/bot"]}"#).unwrap();
        let pending = read_deploy_pending(&paths).unwrap();
        assert_eq!(pending.sha, "abc");
        assert_eq!(pending.files, vec![PathBuf::from("/opt/jarvis/bin/bot")]);
        assert_eq!(pending.job, None);
        assert!(paths.deploy_pending().exists());
    }

    #[test]
    fn failed_deploy_records_job_state_and_clears_pending() {
        let dir = temp_dir("fail");
        let paths = SelfUpdatePaths::new(dir.join("data"));
        let bot = dir.join("bot");
        std::fs::write(&bot, "new").unwrap();
        std::fs::write(prev_path(&bot), "old").unwrap();
        let pending = DeployPending { sha: "abc".into(), files: vec![bot.clone()], job: Some("20261018-101500-00aa".into()) };
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(paths.deploy_pending(), "{}").unwrap();
        fail_deploy(&paths, &pending, "exited during probation (exit status: 1)");
        assert_eq!(std::fs::read_to_string(&bot).unwrap(), "old");
        assert!(!paths.deploy_pending().exists());
        assert_eq!(
            paths.read_job_file("20261018-101500-00aa", "state").as_deref(),
            Some("rolled back: exited during probation (exit status: 1)")
        );
        assert!(paths.take_notices()[0].starts_with("Deploy abc failed probation"));
    }
}
