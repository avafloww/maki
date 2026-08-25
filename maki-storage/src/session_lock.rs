//! Cross-process session locks. One `<id>.lock` file per session inside the
//! sessions dir: the file holds the holder's PID, its mtime is the heartbeat.
//! A session whose lock is fresh and held by another process is open
//! elsewhere and cannot be continued from here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::id::MakiId;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
pub const STALE_AFTER: Duration = Duration::from_secs(5);
pub const OPEN_ELSEWHERE_MSG: &str = "session is open in another terminal; close it there first";

/// Reasons a stored session cannot be continued from this run.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ResumeBlock {
    #[error("session belongs to {0}; cd there and run `maki -c <ID>` from that directory")]
    OtherCwd(String),
    #[error("{OPEN_ELSEWHERE_MSG}")]
    OpenElsewhere,
}

pub fn resume_block(
    session_cwd: &str,
    current_cwd: &str,
    open_elsewhere: bool,
) -> Option<ResumeBlock> {
    if session_cwd != current_cwd {
        return Some(ResumeBlock::OtherCwd(session_cwd.to_owned()));
    }
    open_elsewhere.then_some(ResumeBlock::OpenElsewhere)
}

pub fn lock_path(dir: &Path, id: &MakiId) -> PathBuf {
    dir.join(format!("{id}.lock"))
}

fn holder_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// A lock is fresh while its mtime is within `STALE_AFTER` of `now` (or the
/// clock skew puts it in the future).
fn is_fresh(mtime: SystemTime, now: SystemTime) -> bool {
    match now.duration_since(mtime) {
        Ok(d) => d <= STALE_AFTER,
        Err(_) => true,
    }
}

/// Claim the lock if it is absent, stale, malformed, or ours; never clobber a
/// fresh foreign one. Doubles as the periodic heartbeat.
pub fn heartbeat(dir: &Path, id: &MakiId) -> io::Result<()> {
    let path = lock_path(dir, id);
    let pid = std::process::id();
    if let Some(holder) = holder_pid(&path)
        && holder != pid
        && let Ok(mtime) = fs::metadata(&path).and_then(|m| m.modified())
        && is_fresh(mtime, SystemTime::now())
    {
        return Ok(());
    }
    crate::atomic_write(&path, pid.to_string().as_bytes()).map_err(io::Error::other)
}

/// True when another process holds a fresh lock for the session. A stale lock
/// (its holder crashed) is reclaimed best-effort.
pub fn open_elsewhere(dir: &Path, id: &MakiId) -> bool {
    let path = lock_path(dir, id);
    let (Ok(meta), Some(holder)) = (fs::metadata(&path), holder_pid(&path)) else {
        // Absent or malformed: best-effort reclaim.
        let _ = fs::remove_file(&path);
        return false;
    };
    if holder == std::process::id() {
        return false;
    }
    let Some(mtime) = meta.modified().ok() else {
        return false;
    };
    if !is_fresh(mtime, SystemTime::now()) {
        let _ = fs::remove_file(&path);
        return false;
    }
    true
}

/// Drop the lock if we hold it. Best effort: a foreign lock is left for its
/// staleness window to clear.
pub fn release(dir: &Path, id: &MakiId) {
    let path = lock_path(dir, id);
    if holder_pid(&path) == Some(std::process::id()) {
        let _ = fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;
    use test_case::test_case;

    const HERE: &str = "/here";
    const ELSEWHERE: &str = "/elsewhere";
    /// A pid no live process on this machine has.
    const FAKE_PID: u32 = u32::MAX - 1;

    fn fake_lock(dir: &Path, id: &MakiId) {
        fs::write(lock_path(dir, id), FAKE_PID.to_string()).unwrap();
    }

    fn backdate(path: &Path, past: Duration) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - past)
            .unwrap();
    }

    #[test_case(HERE, HERE, false; "same_cwd_free")]
    #[test_case(HERE, HERE, true; "same_cwd_open_elsewhere")]
    #[test_case(ELSEWHERE, HERE, false; "other_cwd_blocked")]
    #[test_case(ELSEWHERE, HERE, true; "other_cwd_wins_over_lock")]
    fn resume_block_matrix(session_cwd: &str, current_cwd: &str, open: bool) {
        let expected = if session_cwd != current_cwd {
            Some(ResumeBlock::OtherCwd(session_cwd.to_owned()))
        } else {
            open.then_some(ResumeBlock::OpenElsewhere)
        };
        assert_eq!(resume_block(session_cwd, current_cwd, open), expected);
    }

    #[test]
    fn open_elsewhere_display_uses_the_shared_message() {
        assert_eq!(ResumeBlock::OpenElsewhere.to_string(), OPEN_ELSEWHERE_MSG);
    }

    #[test_case(0, true; "now")]
    #[test_case(4, true; "just_under_stale")]
    #[test_case(6, false; "past_stale")]
    fn is_fresh_threshold(age_secs: u64, fresh: bool) {
        let now = SystemTime::now();
        assert_eq!(is_fresh(now - Duration::from_secs(age_secs), now), fresh);
    }

    #[test]
    fn heartbeat_claims_absent_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        heartbeat(dir.path(), &id).unwrap();
        assert_eq!(
            holder_pid(&lock_path(dir.path(), &id)),
            Some(std::process::id())
        );
    }

    #[test]
    fn heartbeat_claims_stale_foreign_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fake_lock(dir.path(), &id);
        backdate(
            &lock_path(dir.path(), &id),
            STALE_AFTER + Duration::from_secs(5),
        );
        heartbeat(dir.path(), &id).unwrap();
        assert_eq!(
            holder_pid(&lock_path(dir.path(), &id)),
            Some(std::process::id())
        );
    }

    #[test]
    fn heartbeat_never_clobbers_fresh_foreign_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fake_lock(dir.path(), &id);
        heartbeat(dir.path(), &id).unwrap();
        assert_eq!(holder_pid(&lock_path(dir.path(), &id)), Some(FAKE_PID));
    }

    #[test]
    fn heartbeat_claims_malformed_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fs::write(lock_path(dir.path(), &id), b"not a pid").unwrap();
        heartbeat(dir.path(), &id).unwrap();
        assert_eq!(
            holder_pid(&lock_path(dir.path(), &id)),
            Some(std::process::id())
        );
    }

    #[test]
    fn open_elsewhere_is_true_for_fresh_foreign_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fake_lock(dir.path(), &id);
        assert!(open_elsewhere(dir.path(), &id));
    }

    #[test]
    fn open_elsewhere_is_false_for_own_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        heartbeat(dir.path(), &id).unwrap();
        assert!(!open_elsewhere(dir.path(), &id));
    }

    #[test]
    fn open_elsewhere_is_false_when_absent() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        assert!(!open_elsewhere(dir.path(), &id));
    }

    #[test]
    fn open_elsewhere_reclaims_stale_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fake_lock(dir.path(), &id);
        backdate(
            &lock_path(dir.path(), &id),
            STALE_AFTER + Duration::from_secs(5),
        );
        assert!(!open_elsewhere(dir.path(), &id));
        assert!(!lock_path(dir.path(), &id).exists());
    }

    #[test]
    fn release_removes_own_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        heartbeat(dir.path(), &id).unwrap();
        release(dir.path(), &id);
        assert!(!lock_path(dir.path(), &id).exists());
    }

    #[test]
    fn release_keeps_foreign_lock() {
        let dir = tempdir().unwrap();
        let id = MakiId::generate();
        fake_lock(dir.path(), &id);
        release(dir.path(), &id);
        assert_eq!(holder_pid(&lock_path(dir.path(), &id)), Some(FAKE_PID));
    }
}
