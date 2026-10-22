use crate::db::Database;
use anyhow::{Result, bail};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Manages persistent memories stored in DB with path-based organization.
///
/// Edit protection uses content-hash CAS (compare-and-swap):
/// `read()` returns a `(version, content)` pair where `version` is a hex
/// digest of the current content. `edit()` requires the caller to pass the
/// version it observed; the edit is rejected if the stored content has
/// changed in between. This replaces the previous per-process `HashSet`
/// scheme, which could bleed state across concurrent MCP callers.
pub struct MemoryManager {
    db: Database,
}

/// Compute a cheap, stable version token for a content blob.
/// Not cryptographic — we only need to detect concurrent mutation, not
/// defend against adversarial collisions. DefaultHasher is in std and free.
fn content_version(content: &str) -> String {
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl MemoryManager {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Expose the hash function so tool layers can format version headers
    /// without reimplementing the algorithm.
    pub fn version_of(content: &str) -> String {
        content_version(content)
    }

    /// Validate a memory path: no traversal, no absolute, no null bytes.
    fn validate_path(path: &str) -> Result<String> {
        let path = path.trim();
        if path.is_empty() {
            bail!("memory path cannot be empty");
        }
        if path.contains('\0') {
            bail!("memory path cannot contain null bytes");
        }
        if path.starts_with('/') || path.starts_with('\\') {
            bail!("memory path cannot be absolute");
        }
        if path.contains("..") {
            bail!("memory path cannot contain '..'");
        }

        // Normalize separators
        let normalized = path.replace('\\', "/");
        // Remove leading ./
        let normalized = normalized.trim_start_matches("./").to_string();
        if normalized.is_empty() {
            bail!("memory path cannot be empty after normalization");
        }

        Ok(normalized)
    }

    pub async fn create(&self, path: &str, content: &str) -> Result<String> {
        let path = Self::validate_path(path)?;

        // Check if already exists
        if self.db.read_memory(&path).await?.is_some() {
            bail!("memory already exists at '{path}'. Use edit to modify it.");
        }

        self.db.create_memory(&path, content).await?;
        Ok(format!("Memory created at '{path}'"))
    }

    /// Read a memory by path. Returns `(version, content)` — callers should
    /// pass `version` back to [`edit`](Self::edit) to guard against
    /// concurrent writes.
    pub async fn read(&self, path: &str) -> Result<(String, String)> {
        let path = Self::validate_path(path)?;

        let entry = self.db.read_memory(&path).await?;
        match entry {
            Some((_id, _path, content, _created, _updated)) => {
                let version = content_version(&content);
                Ok((version, content))
            }
            None => bail!("no memory found at '{path}'"),
        }
    }

    /// Edit a memory using compare-and-swap semantics. `expected_version`
    /// must match the hash of the currently-stored content; otherwise the
    /// edit is rejected so the caller can re-read and retry.
    pub async fn edit(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String> {
        let path = Self::validate_path(path)?;

        // Fetch current content and verify the version matches.
        let entry = self.db.read_memory(&path).await?;
        let (_id, _path, current, _created, _updated) = match entry {
            Some(e) => e,
            None => bail!("no memory found at '{path}'"),
        };
        let current_version = content_version(&current);
        if current_version != expected_version {
            bail!(
                "memory version mismatch at '{path}': expected {expected_version}, \
                 current is {current_version}. Re-read before editing."
            );
        }

        if self.db.update_memory(&path, content).await? {
            Ok(format!("Memory updated at '{path}'"))
        } else {
            bail!("no memory found at '{path}'")
        }
    }

    pub async fn delete(&self, path: &str) -> Result<String> {
        let path = Self::validate_path(path)?;

        if self.db.delete_memory(&path).await? {
            Ok(format!("Memory deleted at '{path}'"))
        } else {
            bail!("no memory found at '{path}'")
        }
    }

    pub async fn list(&self) -> Result<String> {
        let memories = self.db.list_memories().await?;
        if memories.is_empty() {
            return Ok("No memories stored.".to_string());
        }

        let mut out = String::new();
        for (path, created, updated) in &memories {
            out.push_str(&format!("- {path} (created={created}, updated={updated})\n"));
        }
        Ok(out)
    }

    pub async fn search(&self, query: &str) -> Result<String> {
        let results = self.db.search_memories(query).await?;
        if results.is_empty() {
            return Ok(format!("No memories matching '{query}'"));
        }

        let mut out = String::new();
        for (path, content) in &results {
            let snippet = if content.len() > 200 {
                format!("{}...", &content[..200])
            } else {
                content.clone()
            };
            out.push_str(&format!("## {path}\n{snippet}\n\n"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod validate_path_tests {
    use super::MemoryManager;

    #[test]
    fn rejects_empty() {
        assert!(MemoryManager::validate_path("").is_err());
        assert!(MemoryManager::validate_path("   ").is_err());
    }

    #[test]
    fn rejects_null_bytes() {
        assert!(MemoryManager::validate_path("a\0b").is_err());
        assert!(MemoryManager::validate_path("notes/a\0.md").is_err());
    }

    #[test]
    fn rejects_absolute_unix() {
        assert!(MemoryManager::validate_path("/etc/passwd").is_err());
        assert!(MemoryManager::validate_path("/tmp/x.md").is_err());
    }

    #[test]
    fn rejects_absolute_windows() {
        assert!(MemoryManager::validate_path("\\windows\\system32").is_err());
    }

    #[test]
    fn rejects_dotdot_traversal() {
        assert!(MemoryManager::validate_path("../secret").is_err());
        assert!(MemoryManager::validate_path("notes/../../etc/passwd").is_err());
        assert!(MemoryManager::validate_path("..").is_err());
    }

    #[test]
    fn accepts_relative_paths() {
        assert_eq!(
            MemoryManager::validate_path("notes/meeting.md").unwrap(),
            "notes/meeting.md"
        );
        assert_eq!(
            MemoryManager::validate_path("flat.md").unwrap(),
            "flat.md"
        );
    }

    #[test]
    fn normalizes_backslash_separators() {
        assert_eq!(
            MemoryManager::validate_path("notes\\meeting.md").unwrap(),
            "notes/meeting.md"
        );
    }

    #[test]
    fn strips_leading_dot_slash() {
        assert_eq!(
            MemoryManager::validate_path("./notes/a.md").unwrap(),
            "notes/a.md"
        );
    }
}

#[cfg(test)]
mod cas_tests {
    use super::*;
    use crate::db::Database;

    async fn manager() -> MemoryManager {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.init().await.unwrap();
        MemoryManager::new(db)
    }

    #[tokio::test]
    async fn read_returns_version_matching_content() {
        let mm = manager().await;
        mm.create("a.md", "hello").await.unwrap();
        let (ver, content) = mm.read("a.md").await.unwrap();
        assert_eq!(content, "hello");
        assert_eq!(ver, MemoryManager::version_of("hello"));
    }

    #[tokio::test]
    async fn edit_with_correct_version_succeeds() {
        let mm = manager().await;
        mm.create("a.md", "v1").await.unwrap();
        let (ver, _) = mm.read("a.md").await.unwrap();
        mm.edit("a.md", &ver, "v2").await.unwrap();
        let (_, content) = mm.read("a.md").await.unwrap();
        assert_eq!(content, "v2");
    }

    #[tokio::test]
    async fn edit_with_stale_version_is_rejected() {
        let mm = manager().await;
        mm.create("a.md", "v1").await.unwrap();
        let (ver_a, _) = mm.read("a.md").await.unwrap();

        // Concurrent writer changes content. Once v2 is stored, the old
        // version token becomes stale.
        mm.edit("a.md", &ver_a, "v2").await.unwrap();

        // Attempting to reuse the now-stale v1 version must fail.
        let err = mm.edit("a.md", &ver_a, "v3").await.unwrap_err();
        assert!(
            err.to_string().contains("version mismatch"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn edit_without_prior_read_is_rejected_via_wrong_version() {
        let mm = manager().await;
        mm.create("a.md", "v1").await.unwrap();
        // Caller makes up a version — should fail.
        let err = mm.edit("a.md", "deadbeef", "v2").await.unwrap_err();
        assert!(err.to_string().contains("version mismatch"));
    }
}
