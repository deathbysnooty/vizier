//! Deleting old uploads so they cannot fill the disk.
//!
//! Every file the bot is handed (a Discord attachment, a GIF link it fetched,
//! a voice note) is written to `<workspace>/uploads/<id>/` and, until this
//! existed, kept forever: 24 GB on the MLCI server by November 2026, growing
//! by about 240 MB a day, which filled the disk. Nothing reads an old upload
//! back with the chat history; one is only reopened if the bot is asked to
//! look at that exact file again, or if a memory points at it. So uploads
//! older than `VIZIER_UPLOADS_KEEP_DAYS` (default 30, 0 = never delete) are
//! removed, except any file a memory still points at.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::schema::VizierAttachmentContent;
use crate::storage::{VizierStorage, agent::AgentStorage, memory::MemoryStorage};

/// Uploads younger than this many days are kept; 0 turns the sweep off.
pub const KEEP_DAYS_VAR: &str = "VIZIER_UPLOADS_KEEP_DAYS";
const DEFAULT_KEEP_DAYS: u64 = 30;
/// The first sweep waits this long after boot, so it never competes with startup.
const FIRST_WAIT: Duration = Duration::from_secs(10 * 60);
/// Then once every this long.
const EVERY: Duration = Duration::from_secs(6 * 3600);

pub fn keep_days() -> u64 {
    std::env::var(KEEP_DAYS_VAR).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_KEEP_DAYS)
}

/// What one sweep removed.
#[derive(Debug, Default, PartialEq)]
pub struct Swept {
    pub folders: usize,
    pub bytes: u64,
    pub kept_for_memories: usize,
}

/// The upload ids any agent's memory points at (`/api/v1/files/<id>`).
async fn ids_in_memories(storage: &VizierStorage) -> anyhow::Result<HashSet<String>> {
    let mut out = HashSet::new();
    for (agent_id, _) in storage.list_agents().await? {
        for memory in storage.get_all_agent_memory(agent_id).await? {
            for att in &memory.attachments {
                if let VizierAttachmentContent::Local(url) = &att.content {
                    out.insert(url.trim_start_matches("/api/v1/files/").to_string());
                }
            }
        }
    }
    Ok(out)
}

fn size_of(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Removes every upload folder last changed before `cutoff` that is not in
/// `protected`. A folder that cannot be read or removed is skipped, not fatal.
pub fn sweep_dir(uploads: &Path, cutoff: SystemTime, protected: &HashSet<String>) -> Swept {
    let mut swept = Swept::default();
    let Ok(entries) = std::fs::read_dir(uploads) else { return swept };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let Ok(changed) = meta.modified() else { continue };
        if changed >= cutoff {
            continue;
        }
        if protected.contains(&entry.file_name().to_string_lossy().to_string()) {
            swept.kept_for_memories += 1;
            continue;
        }
        let bytes = size_of(&path);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                swept.folders += 1;
                swept.bytes += bytes;
            }
            Err(err) => tracing::warn!("uploads: couldn't remove {}: {}", path.display(), err),
        }
    }
    swept
}

/// One sweep of `<workspace>/uploads`.
pub async fn sweep_once(workspace: &str, storage: &VizierStorage, keep_days: u64) -> anyhow::Result<Swept> {
    let protected = ids_in_memories(storage).await?;
    let uploads = PathBuf::from(workspace).join("uploads");
    let cutoff = SystemTime::now() - Duration::from_secs(keep_days * 86_400);
    Ok(tokio::task::spawn_blocking(move || sweep_dir(&uploads, cutoff, &protected)).await?)
}

/// Runs for the life of the process: a sweep shortly after boot, then every few hours.
pub async fn run(workspace: String, storage: VizierStorage) {
    tokio::time::sleep(FIRST_WAIT).await;
    loop {
        let days = keep_days();
        if days == 0 {
            tracing::info!("uploads: {}=0, so old uploads are kept", KEEP_DAYS_VAR);
        } else {
            match sweep_once(&workspace, &storage, days).await {
                Ok(s) if s.folders > 0 || s.kept_for_memories > 0 => tracing::info!(
                    "uploads: removed {} older than {} days ({:.1} MB); kept {} a memory points at",
                    s.folders,
                    days,
                    s.bytes as f64 / 1_048_576.0,
                    s.kept_for_memories
                ),
                Ok(_) => tracing::debug!("uploads: nothing older than {} days", days),
                Err(err) => tracing::warn!("uploads: sweep failed: {}", err),
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(root: &Path, id: &str, bytes: usize) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn old_folders_go_new_and_remembered_ones_stay() {
        let tmp = tempfile::tempdir().unwrap();
        folder(tmp.path(), "old", 10);
        folder(tmp.path(), "kept", 20);
        // A cutoff in the future makes both folders "old".
        let cutoff = SystemTime::now() + Duration::from_secs(60);
        let protected: HashSet<String> = ["kept".to_string()].into();
        let s = sweep_dir(tmp.path(), cutoff, &protected);
        assert_eq!(s, Swept { folders: 1, bytes: 10, kept_for_memories: 1 });
        assert!(!tmp.path().join("old").exists());
        assert!(tmp.path().join("kept").exists());
    }

    #[test]
    fn nothing_younger_than_the_cutoff_is_touched() {
        let tmp = tempfile::tempdir().unwrap();
        folder(tmp.path(), "fresh", 10);
        let cutoff = SystemTime::now() - Duration::from_secs(86_400);
        let s = sweep_dir(tmp.path(), cutoff, &HashSet::new());
        assert_eq!(s, Swept::default());
        assert!(tmp.path().join("fresh").exists());
    }

    #[test]
    fn a_missing_uploads_folder_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(sweep_dir(&tmp.path().join("nope"), SystemTime::now(), &HashSet::new()), Swept::default());
    }
}
