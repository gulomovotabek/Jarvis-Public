use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GIT_SHA: &str = env!("JARVIS_GIT_SHA");

const JOB_ENV_PASSTHROUGH: &[&str] = &["HOME", "PATH", "USER", "LOGNAME", "LANG", "CLAUDE_CODE_BIN", "CLAUDE_CODE_MODEL"];

pub struct SelfUpdatePaths {
    data_dir: PathBuf,
}

impl SelfUpdatePaths {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }

    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://data/jarvis.db".to_string());
        Self::new(data_dir_from_database_url(&database_url))
    }

    pub fn restart_flag(&self) -> PathBuf {
        self.data_dir.join("restart_requested")
    }

    pub fn notices_dir(&self) -> PathBuf {
        self.data_dir.join("notices")
    }

    pub fn deploy_pending(&self) -> PathBuf {
        self.data_dir.join("deploy_pending")
    }

    pub fn self_improve_dir(&self) -> PathBuf {
        self.data_dir.join("self_improve")
    }

    pub fn write_notice(&self, text: &str) -> Result<PathBuf> {
        let dir = self.notices_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let name = format!("{}-{}", unique_stamp(), std::process::id());
        let tmp = dir.join(format!(".{name}.tmp"));
        let path = dir.join(format!("{name}.txt"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    pub fn take_notices(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.notices_dir()) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "txt"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|p| {
                let text = std::fs::read_to_string(&p).ok();
                std::fs::remove_file(&p).ok()?;
                text.filter(|t| !t.trim().is_empty())
            })
            .collect()
    }

    pub fn request_restart(&self, reason: &str) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(self.restart_flag(), reason)?;
        Ok(())
    }

    pub fn restart_requested(&self) -> bool {
        self.restart_flag().exists()
    }

    pub fn take_restart_reason(&self) -> Option<String> {
        let path = self.restart_flag();
        let reason = std::fs::read_to_string(&path).ok()?;
        std::fs::remove_file(&path).ok()?;
        Some(reason.trim().to_string())
    }

    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.self_improve_dir().join(id)
    }

    pub fn write_job_state(&self, id: &str, state: &str) -> Result<()> {
        let dir = self.job_dir(id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let tmp = dir.join(".state.tmp");
        std::fs::write(&tmp, state)?;
        std::fs::rename(&tmp, dir.join("state"))?;
        Ok(())
    }

    pub fn read_job_file(&self, id: &str, name: &str) -> Option<String> {
        std::fs::read_to_string(self.job_dir(id).join(name)).ok()
    }

    pub fn recent_jobs(&self, limit: usize) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.self_improve_dir()) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|id| is_job_id(id))
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.truncate(limit);
        ids
    }

    pub fn spawn_job(&self, script: &Path, args: &[&str]) -> Result<()> {
        let id = args.get(1).context("job args need <mode> <job-id>")?;
        let dir = self.job_dir(id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log"))?;
        let prod_dir = std::env::current_dir()?;
        let script = prod_dir.join(script);
        let mut cmd = Command::new(&script);
        cmd.args(args)
            .env_clear()
            .envs(std::env::vars().filter(|(k, _)| JOB_ENV_PASSTHROUGH.contains(&k.as_str()) || k.starts_with("SELF_IMPROVE_")))
            .env("JARVIS_DATA_DIR", std::path::absolute(&self.data_dir)?)
            .env("JARVIS_PROD", &prod_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0);
        let mut child = cmd.spawn().with_context(|| format!("spawn {}", script.display()))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

pub fn self_improve_script() -> PathBuf {
    std::env::var("SELF_IMPROVE_SCRIPT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bin/self-improve.sh"))
}

pub fn new_job_id() -> String {
    let now = chrono::Utc::now();
    format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), now.timestamp_subsec_nanos() & 0xffff)
}

pub fn is_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn is_safe_git_ref(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with(['-', '/'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
}

pub fn data_dir_from_database_url(database_url: &str) -> PathBuf {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    let path = path.split('?').next().unwrap_or(path);
    Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn unique_stamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:010}{:09}", now.as_secs(), now.subsec_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> SelfUpdatePaths {
        let dir = std::env::temp_dir().join(format!("jarvis-self-update-{tag}-{}", unique_stamp()));
        SelfUpdatePaths::new(dir)
    }

    #[test]
    fn notices_are_returned_in_order_once_and_skip_partial_writes() {
        let paths = temp_paths("notices");
        paths.write_notice("first").unwrap();
        paths.write_notice("second").unwrap();
        std::fs::write(paths.notices_dir().join(".partial.tmp"), "half").unwrap();
        assert_eq!(paths.take_notices(), vec!["first".to_string(), "second".to_string()]);
        assert!(paths.take_notices().is_empty());
        assert!(paths.notices_dir().join(".partial.tmp").exists());
    }

    #[test]
    fn restart_request_is_consumed_once() {
        let paths = temp_paths("restart");
        assert!(!paths.restart_requested());
        paths.request_restart("deploy abc123\n").unwrap();
        assert!(paths.restart_requested());
        assert_eq!(paths.take_restart_reason().as_deref(), Some("deploy abc123"));
        assert!(!paths.restart_requested());
        assert_eq!(paths.take_restart_reason(), None);
    }

    #[test]
    fn job_state_and_listing_newest_first() {
        let paths = temp_paths("jobs");
        paths.write_job_state("20261018-101500-00aa", "queued").unwrap();
        paths.write_job_state("20261018-101500-00aa", "agent running").unwrap();
        paths.write_job_state("20261018-120000-0b0b", "noop").unwrap();
        std::fs::create_dir_all(paths.self_improve_dir().join(".hidden")).unwrap();
        std::fs::write(paths.self_improve_dir().join(".lock"), "").unwrap();
        assert_eq!(paths.recent_jobs(10), vec!["20261018-120000-0b0b".to_string(), "20261018-101500-00aa".to_string()]);
        assert_eq!(paths.recent_jobs(1).len(), 1);
        assert_eq!(paths.read_job_file("20261018-101500-00aa", "state").as_deref(), Some("agent running"));
        assert!(paths.read_job_file("20261018-101500-00aa", "summary.md").is_none());
    }

    #[test]
    fn job_ids_and_refs_are_validated() {
        assert!(is_job_id(&new_job_id()));
        assert!(!is_job_id("../etc"));
        assert!(!is_job_id("-rf"));
        assert!(!is_job_id(""));
        assert!(is_safe_git_ref("jarvis/20261018-101500-00aa"));
        assert!(is_safe_git_ref("main"));
        for bad in ["", "-x", "/abs", "a/../b", "a//b", "a/", "a.", "a b", "a;rm", "a~1", "a^", "a:b"] {
            assert!(!is_safe_git_ref(bad), "{bad}");
        }
    }

    #[test]
    fn spawn_job_runs_script_with_clean_env_and_logs() {
        let paths = temp_paths("spawn");
        let dir = std::env::temp_dir().join(format!("jarvis-spawn-script-{}", unique_stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("job.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"$1 $2 data=$JARVIS_DATA_DIR leak=${CARGO_MANIFEST_DIR:-none}\"\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        paths.spawn_job(&script, &["improve", "20261018-101500-00aa"]).unwrap();
        let log = paths.job_dir("20261018-101500-00aa").join("log");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&log).unwrap_or_default().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("improve 20261018-101500-00aa data=/"), "{text}");
        assert!(text.trim_end().ends_with("leak=none"), "{text}");
    }

    #[test]
    fn data_dir_follows_database_url() {
        assert_eq!(data_dir_from_database_url("sqlite://data/jarvis.db"), PathBuf::from("data"));
        assert_eq!(data_dir_from_database_url("sqlite:///opt/jarvis/data/jarvis.db?mode=rwc"), PathBuf::from("/opt/jarvis/data"));
        assert_eq!(data_dir_from_database_url("sqlite:jarvis.db"), PathBuf::from("data"));
    }
}
