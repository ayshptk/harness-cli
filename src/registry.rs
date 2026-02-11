use std::path::PathBuf;

use crate::models::ModelRegistry;

/// URL to fetch the canonical models.toml from GitHub.
const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/ayshptk/harness/main/models.toml";

/// Cache TTL in seconds (24 hours).
const TTL_SECS: u64 = 86400;

/// HTTP request timeout in seconds.
const FETCH_TIMEOUT_SECS: u64 = 5;

/// Path to the cached registry: `~/.harness/models.toml`.
pub fn canonical_path() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".harness").join("models.toml"))
}

/// Load the canonical model registry.
///
/// Resolution order:
/// 1. Load from `~/.harness/models.toml` if it exists and is fresh (< 24h old).
/// 2. If missing or stale, attempt to fetch from GitHub and cache.
/// 3. If fetch fails, use cached version (even if stale).
/// 4. If no cache at all, fall back to the builtin registry.
///
/// This function **never** fails — it always returns a usable registry.
pub fn load_canonical() -> ModelRegistry {
    let path = match canonical_path() {
        Some(p) => p,
        None => {
            tracing::debug!("cannot determine home directory, using builtin registry");
            return ModelRegistry::builtin();
        }
    };

    // If the file exists and is fresh, use it.
    if path.exists() && !is_stale(&path) {
        if let Some(reg) = load_from_disk(&path) {
            return reg;
        }
    }

    // Try to fetch and cache a fresh copy.
    match fetch_and_cache(&path) {
        Ok(reg) => return reg,
        Err(e) => {
            tracing::debug!("failed to fetch models registry: {e}");
        }
    }

    // Fall back to stale cache.
    if path.exists() {
        if let Some(reg) = load_from_disk(&path) {
            tracing::debug!("using stale cached registry");
            return reg;
        }
    }

    // Ultimate fallback: builtin.
    tracing::debug!("using builtin registry");
    ModelRegistry::builtin()
}

/// Force-fetch the registry from GitHub and cache it.
/// Returns a human-readable status message.
pub fn force_update() -> Result<String, String> {
    let path = canonical_path().ok_or("cannot determine home directory")?;
    match fetch_and_cache(&path) {
        Ok(_) => Ok(format!("Updated registry at {}", path.display())),
        Err(e) => Err(format!("failed to fetch: {e}")),
    }
}

/// Check if the cached file is older than `TTL_SECS`.
fn is_stale(path: &std::path::Path) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let modified = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    age.as_secs() > TTL_SECS
}

/// Load and parse a registry from disk, returning `None` on any error.
fn load_from_disk(path: &std::path::Path) -> Option<ModelRegistry> {
    let content = std::fs::read_to_string(path).ok()?;
    match ModelRegistry::from_toml(&content) {
        Ok(reg) => Some(reg),
        Err(e) => {
            tracing::warn!("failed to parse cached registry at {}: {e}", path.display());
            None
        }
    }
}

/// Fetch from GitHub, parse, and atomically write to disk.
fn fetch_and_cache(path: &std::path::Path) -> Result<ModelRegistry, String> {
    let body = fetch_registry_content()?;
    let reg = ModelRegistry::from_toml(&body)?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }

    // Atomic write: write to .tmp, then rename.
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &body).map_err(|e| format!("write failed: {e}"))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("rename failed: {e}"))?;

    tracing::debug!("cached registry at {}", path.display());
    Ok(reg)
}

/// HTTP GET the registry content.
fn fetch_registry_content() -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS)))
        .build()
        .new_agent();
    let body = agent
        .get(REGISTRY_URL)
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed to read response body: {e}"))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_path_is_under_home() {
        if let Some(path) = canonical_path() {
            assert!(path.to_string_lossy().contains(".harness"));
            assert!(path.to_string_lossy().ends_with("models.toml"));
        }
    }

    #[test]
    fn is_stale_missing_file() {
        assert!(is_stale(std::path::Path::new("/nonexistent/file")));
    }

    #[test]
    fn load_from_disk_valid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[models.test]
description = "Test Model"
provider = "test"
claude = "test-id"
"#,
        )
        .unwrap();
        let reg = load_from_disk(tmp.path());
        assert!(reg.is_some());
        assert!(reg.unwrap().models.contains_key("test"));
    }

    #[test]
    fn load_from_disk_invalid_returns_none() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "{{{{ not toml").unwrap();
        assert!(load_from_disk(tmp.path()).is_none());
    }

    #[test]
    fn load_canonical_returns_something() {
        // This should always succeed, at minimum returning the builtin.
        let reg = load_canonical();
        assert!(!reg.models.is_empty());
    }
}
