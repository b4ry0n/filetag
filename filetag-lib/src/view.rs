#![cfg(unix)]

use std::collections::HashSet;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Generate symlink views for a list of file paths.
/// Each symlink in `output_dir` points (relatively) to the actual file under `root`.
pub fn generate(root: &Path, paths: &[String], output_dir: &Path) -> Result<ViewStats> {
    std::fs::create_dir_all(output_dir)?;
    cleanup_broken_symlinks(output_dir)?;

    let mut stats = ViewStats::default();

    for rel_path in paths {
        let abs_target = root.join(rel_path);
        if !abs_target.exists() {
            stats.missing += 1;
            continue;
        }

        let link_name = path_to_link_name(rel_path);
        let link_path = output_dir.join(&link_name);

        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            stats.skipped += 1;
            continue;
        }

        let rel_target = relative_path(&abs_target, output_dir)?;
        unix_fs::symlink(&rel_target, &link_path)
            .with_context(|| format!("creating symlink {}", link_path.display()))?;
        stats.created += 1;
    }

    cleanup_empty_dirs(output_dir)?;
    Ok(stats)
}

#[derive(Default)]
pub struct ViewStats {
    pub created: usize,
    pub skipped: usize,
    pub missing: usize,
    #[allow(dead_code)]
    pub broken_removed: usize,
    #[allow(dead_code)]
    pub empty_dirs_removed: usize,
}

/// Convert a relative path like `Music/Album/song.mp3` to a flat symlink name
/// like `Music__Album__song.mp3`, avoiding collisions.
fn path_to_link_name(rel_path: &str) -> String {
    let path = Path::new(rel_path);
    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    if components.len() <= 1 {
        return truncate_filename(rel_path, 255);
    }

    // Join all components with __
    let prefix_parts = &components[..components.len() - 1];
    let filename = components[components.len() - 1];
    let prefix = prefix_parts.join("__");
    let full_name = format!("{}__{}", prefix, filename);

    truncate_filename(&full_name, 255)
}

fn truncate_filename(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_string();
    }

    // Split name and extension
    let (stem, ext) = match name.rfind('.') {
        Some(dot_pos) if dot_pos > 0 => (&name[..dot_pos], &name[dot_pos..]),
        _ => (name, ""),
    };

    let available = max_bytes.saturating_sub(ext.len());
    let available = available.max(5);

    let truncated_stem = &stem[..stem.len().min(available)];
    format!("{}{}", truncated_stem, ext)
}

/// Compute a relative path from `base` directory to `target` file.
fn relative_path(target: &Path, base: &Path) -> Result<PathBuf> {
    let target = std::fs::canonicalize(target)
        .or_else(|_| Ok::<PathBuf, std::io::Error>(target.to_path_buf()))?;
    let base = std::fs::canonicalize(base)
        .or_else(|_| Ok::<PathBuf, std::io::Error>(base.to_path_buf()))?;

    let target_components: Vec<_> = target.components().collect();
    let base_components: Vec<_> = base.components().collect();

    let common = target_components
        .iter()
        .zip(base_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut rel = PathBuf::new();
    for _ in common..base_components.len() {
        rel.push("..");
    }
    for comp in &target_components[common..] {
        rel.push(comp);
    }
    Ok(rel)
}

/// Remove broken symlinks from a directory (non-recursive first level).
fn cleanup_broken_symlinks(dir: &Path) -> Result<usize> {
    let mut removed = 0;
    if !dir.is_dir() {
        return Ok(0);
    }
    // Walk all entries recursively
    for entry in walkdir::WalkDir::new(dir).min_depth(1) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        // Check if it's a symlink that's broken
        if path.symlink_metadata().is_ok() && !path.exists() {
            std::fs::remove_file(path).ok();
            removed += 1;
        }
    }
    Ok(removed)
}

/// Remove empty directories (bottom-up).
fn cleanup_empty_dirs(dir: &Path) -> Result<usize> {
    let mut removed = 0;
    let mut seen = HashSet::new();
    loop {
        let mut found_empty = false;
        for entry in walkdir::WalkDir::new(dir).min_depth(1).contents_first(true) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.is_dir()
                && !seen.contains(path)
                && std::fs::read_dir(path)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false)
            {
                std::fs::remove_dir(path).ok();
                seen.insert(path.to_path_buf());
                removed += 1;
                found_empty = true;
            }
        }
        if !found_empty {
            break;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_link_name() {
        assert_eq!(path_to_link_name("song.mp3"), "song.mp3");
        assert_eq!(
            path_to_link_name("Music/Album/song.mp3"),
            "Music__Album__song.mp3"
        );
    }

    #[test]
    fn test_truncate() {
        let long = "a".repeat(300) + ".mp3";
        let result = truncate_filename(&long, 255);
        assert!(result.len() <= 255);
        assert!(result.ends_with(".mp3"));
    }
}
