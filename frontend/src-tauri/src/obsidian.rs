//! Export meeting notes (summary + transcript) as Markdown into an Obsidian vault.
//!
//! Notes are plain files written into `<vault>/<folder>/`; Obsidian picks them up
//! automatically. Re-exporting the same meeting overwrites its note (matched via
//! the `meetily_id` frontmatter field) instead of creating duplicates.

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use tauri::AppHandle;

#[derive(Debug, Serialize)]
pub struct ObsidianVault {
    pub path: String,
    pub name: String,
    pub open: bool,
}

#[derive(Debug, Deserialize)]
struct ObsidianConfig {
    #[serde(default)]
    vaults: HashMap<String, ObsidianConfigVault>,
}

#[derive(Debug, Deserialize)]
struct ObsidianConfigVault {
    path: String,
    #[serde(default)]
    open: bool,
}

/// Location of Obsidian's own config file, which lists the known vaults.
fn obsidian_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return dirs::home_dir().map(|h| h.join("Library/Application Support/obsidian/obsidian.json"));
    #[cfg(target_os = "windows")]
    return dirs::config_dir().map(|d| d.join("obsidian").join("obsidian.json"));
    #[cfg(target_os = "linux")]
    return dirs::config_dir().map(|d| d.join("obsidian").join("obsidian.json"));
    #[allow(unreachable_code)]
    None
}

/// List vaults registered in the local Obsidian install (empty if Obsidian isn't installed).
#[tauri::command]
pub async fn obsidian_detect_vaults() -> Result<Vec<ObsidianVault>, String> {
    let Some(config_path) = obsidian_config_path() else {
        return Ok(Vec::new());
    };
    let Ok(raw) = std::fs::read_to_string(&config_path) else {
        return Ok(Vec::new());
    };
    let config: ObsidianConfig = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse Obsidian config: {}", e))?;

    let mut vaults: Vec<ObsidianVault> = config
        .vaults
        .into_values()
        .filter(|v| Path::new(&v.path).is_dir())
        .map(|v| ObsidianVault {
            name: Path::new(&v.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| v.path.clone()),
            path: v.path,
            open: v.open,
        })
        .collect();
    vaults.sort_by(|a, b| b.open.cmp(&a.open).then(a.name.cmp(&b.name)));
    Ok(vaults)
}

/// Let the user pick the vault folder with a native dialog.
#[tauri::command]
pub async fn obsidian_select_vault(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = app.dialog().file().set_title("Select your Obsidian vault folder");
    if let Some(first) = obsidian_detect_vaults().await?.into_iter().next() {
        if let Some(parent) = Path::new(&first.path).parent() {
            dialog = dialog.set_directory(parent);
        }
    }

    Ok(dialog.blocking_pick_folder().map(|p| p.to_string()))
}

/// Replace characters Obsidian or the filesystem don't allow in note names.
fn sanitize_file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    let stem: String = trimmed.chars().take(120).collect();
    if stem.is_empty() {
        "Meeting".to_string()
    } else {
        stem
    }
}

/// Reject absolute paths and `..` so the note always lands inside the vault.
fn relative_folder(folder: &str) -> Result<PathBuf, String> {
    let folder = folder.trim().trim_matches('/').trim_matches('\\');
    let path = PathBuf::from(folder);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("Invalid folder inside the vault: '{}'", folder));
    }
    Ok(path)
}

fn belongs_to_meeting(path: &Path, meeting_id: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains(&format!("meetily_id: \"{}\"", meeting_id)))
        .unwrap_or(false)
}

/// Write the note and return its absolute path.
#[tauri::command]
pub async fn obsidian_export_note(
    vault_path: String,
    folder: String,
    file_name: String,
    meeting_id: String,
    content: String,
) -> Result<String, String> {
    let vault = PathBuf::from(&vault_path);
    if !vault.is_dir() {
        return Err(format!("Obsidian vault not found: {}", vault_path));
    }

    let target_dir = vault.join(relative_folder(&folder)?);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create folder {}: {}", target_dir.display(), e))?;

    // Same title + date as another meeting: suffix instead of overwriting its note.
    let stem = sanitize_file_stem(&file_name);
    let mut target = target_dir.join(format!("{}.md", stem));
    let mut n = 2;
    while target.exists() && !belongs_to_meeting(&target, &meeting_id) {
        target = target_dir.join(format!("{} ({}).md", stem, n));
        n += 1;
    }

    if target.exists() {
        warn!("Overwriting existing Obsidian note for meeting {}", meeting_id);
    }
    std::fs::write(&target, content)
        .map_err(|e| format!("Failed to write {}: {}", target.display(), e))?;

    info!("📝 Exported meeting {} to Obsidian: {}", meeting_id, target.display());
    Ok(target.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_forbidden_characters() {
        assert_eq!(sanitize_file_stem("2026-09-25 Sync: A/B [draft]"), "2026-09-25 Sync- A-B -draft-");
        assert_eq!(sanitize_file_stem("  ..  "), "Meeting");
    }

    #[test]
    fn rejects_folders_escaping_the_vault() {
        assert!(relative_folder("../outside").is_err());
        assert!(relative_folder("notes/../../x").is_err());
        assert_eq!(relative_folder("/meeting-notes/").unwrap(), PathBuf::from("meeting-notes"));
        assert_eq!(relative_folder("work/meeting-notes").unwrap(), PathBuf::from("work/meeting-notes"));
    }

    #[test]
    fn suffixes_note_of_a_different_meeting() {
        let vault = std::env::temp_dir().join(format!("meetily-obsidian-test-{}", std::process::id()));
        std::fs::create_dir_all(&vault).unwrap();
        let vault_str = vault.to_string_lossy().to_string();
        let export = |id: &str| {
            tauri::async_runtime::block_on(obsidian_export_note(
                vault_str.clone(),
                "meeting-notes".into(),
                "2026-09-25 Standup".into(),
                id.into(),
                format!("---\nmeetily_id: \"{}\"\n---\n", id),
            ))
            .unwrap()
        };

        let first = export("a");
        assert_eq!(export("a"), first, "re-export overwrites the same note");
        assert!(export("b").ends_with("2026-09-25 Standup (2).md"));

        std::fs::remove_dir_all(&vault).unwrap();
    }
}
