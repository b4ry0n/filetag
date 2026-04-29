//! Global database registry at `~/.config/filetag/databases.json`.
//!
//! Tracks all known filetag database roots so that `--all-dbs` can query
//! across unrelated directory trees.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const CONFIG_DIR: &str = "filetag";
const REGISTRY_FILE: &str = "databases.json";

/// Serialised form of `~/.config/filetag/databases.json`.
#[derive(Serialize, Deserialize, Default)]
pub struct Registry {
    /// Absolute, canonicalised paths to registered database roots.
    pub databases: Vec<String>,
}

/// Path to `~/.config/filetag/databases.json`.
fn registry_path() -> Result<PathBuf> {
    let config = dirs::config_dir().context("could not determine config directory (~/.config)")?;
    Ok(config.join(CONFIG_DIR).join(REGISTRY_FILE))
}

/// Load the registry, returning an empty one if the file doesn't exist.
pub fn load() -> Result<Registry> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(Registry::default());
    }
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let reg: Registry =
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
    Ok(reg)
}

/// Save the registry to disk, creating directories as needed.
fn save(reg: &Registry) -> Result<()> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(reg)?;
    std::fs::write(&path, data).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Register a database root. Canonicalizes the path. No-op if already registered.
pub fn add(root: &Path) -> Result<bool> {
    let canonical = std::fs::canonicalize(root)
        .with_context(|| format!("canonicalizing {}", root.display()))?;
    let s = canonical.to_string_lossy().into_owned();

    let mut reg = load()?;
    if reg.databases.contains(&s) {
        return Ok(false);
    }
    reg.databases.push(s);
    reg.databases.sort();
    save(&reg)?;
    Ok(true)
}

/// Remove a database root from the registry.
pub fn remove(root: &Path) -> Result<bool> {
    let canonical = std::fs::canonicalize(root).unwrap_or(root.to_path_buf());
    let s = canonical.to_string_lossy().into_owned();

    let mut reg = load()?;
    let before = reg.databases.len();
    reg.databases.retain(|d| d != &s);
    if reg.databases.len() == before {
        return Ok(false);
    }
    save(&reg)?;
    Ok(true)
}

/// Remove entries whose `.filetag/db.sqlite3` no longer exists.
pub fn prune() -> Result<Vec<String>> {
    let mut reg = load()?;
    let mut pruned = Vec::new();
    reg.databases.retain(|d| {
        let db_path = PathBuf::from(d).join(".filetag").join("db.sqlite3");
        if db_path.is_file() {
            true
        } else {
            pruned.push(d.clone());
            false
        }
    });
    if !pruned.is_empty() {
        save(&reg)?;
    }
    Ok(pruned)
}

/// List all registered database roots.
pub fn list() -> Result<Vec<String>> {
    let reg = load()?;
    Ok(reg.databases)
}
