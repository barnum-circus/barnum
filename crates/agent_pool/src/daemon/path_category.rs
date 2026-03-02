//! Path categorization for filesystem events.
//!
//! Categorizes filesystem paths to determine what kind of entity they represent
//! (worker ready file, response file, submission request, etc.).
//!
//! The categorization takes both path AND event kind into account, only returning
//! a category when the event is meaningful for that path type. This avoids race
//! conditions where we might try to read a file before it's fully written.

use std::path::Path;

use notify::event::EventKind;

use crate::constants::{READY_SUFFIX, REQUEST_SUFFIX, WORKER_RESPONSE_SUFFIX};
use crate::fs::is_write_complete;

/// Category of a filesystem path.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PathCategory {
    // -------------------------------------------------------------------------
    // Anonymous worker protocol (flat files)
    // -------------------------------------------------------------------------
    /// Worker ready file: `agents/<uuid>.ready.json`
    WorkerReady {
        /// The worker's UUID.
        id: String,
    },
    /// Worker response file: `agents/<uuid>.response.json`
    WorkerResponse {
        /// The worker's UUID.
        id: String,
    },

    // -------------------------------------------------------------------------
    // Submission protocol
    // -------------------------------------------------------------------------
    /// Submission request file: `submissions/<id>.request.json`
    SubmissionRequest {
        /// The submission's ID.
        id: String,
    },
}

/// Categorize a filesystem event (path + event kind).
///
/// Returns `Some(category)` only when the event is meaningful for that path type:
/// - `WorkerReady`: only on write complete (worker signaling availability)
/// - `WorkerResponse`: only on write complete (response ready to read)
/// - `SubmissionRequest`: only on write complete (request ready to read)
///
/// This approach centralizes the "when is this ready?" logic, avoiding race
/// conditions where we might process events before files are fully written.
#[must_use]
pub(super) fn categorize(
    path: &Path,
    event_kind: EventKind,
    agents_dir: &Path,
    submissions_dir: &Path,
) -> Option<PathCategory> {
    categorize_under_agents(path, event_kind, agents_dir)
        .or_else(|| categorize_under_submissions(path, event_kind, submissions_dir))
}

fn categorize_under_agents(
    path: &Path,
    event_kind: EventKind,
    agents_dir: &Path,
) -> Option<PathCategory> {
    // Only process when write is complete
    if !is_write_complete(event_kind) {
        return None;
    }

    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Must be exactly one component (flat file)
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    // Anonymous workers: flat files
    if let Some(id) = filename.strip_suffix(READY_SUFFIX) {
        return Some(PathCategory::WorkerReady { id: id.to_string() });
    }
    if let Some(id) = filename.strip_suffix(WORKER_RESPONSE_SUFFIX) {
        return Some(PathCategory::WorkerResponse { id: id.to_string() });
    }

    None
}

fn categorize_under_submissions(
    path: &Path,
    event_kind: EventKind,
    submissions_dir: &Path,
) -> Option<PathCategory> {
    // Only process when write is complete
    if !is_write_complete(event_kind) {
        return None;
    }

    let relative = path.strip_prefix(submissions_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Must be exactly one component (flat file)
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    if let Some(id) = filename.strip_suffix(REQUEST_SUFFIX) {
        return Some(PathCategory::SubmissionRequest { id: id.to_string() });
    }

    // SubmissionResponse is written by the daemon, we don't need to react to it
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use notify::event::CreateKind;

    use super::*;

    const TEST_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn agents() -> PathBuf {
        PathBuf::from("/pool/agents")
    }

    fn submissions() -> PathBuf {
        PathBuf::from("/pool/submissions")
    }

    /// The canonical "file written" event for the current platform.
    /// On Linux, this is Close(Write). On macOS, this is Create(File).
    #[cfg(target_os = "linux")]
    fn file_written() -> EventKind {
        use notify::event::{AccessKind, AccessMode};
        EventKind::Access(AccessKind::Close(AccessMode::Write))
    }

    #[cfg(target_os = "macos")]
    fn file_written() -> EventKind {
        EventKind::Create(CreateKind::File)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn file_written() -> EventKind {
        use notify::event::{AccessKind, AccessMode};
        EventKind::Access(AccessKind::Close(AccessMode::Write))
    }

    fn folder_created() -> EventKind {
        EventKind::Create(CreateKind::Folder)
    }

    // =========================================================================
    // Worker ready file
    // =========================================================================

    #[test]
    fn worker_ready_on_file_written() {
        let path = PathBuf::from(format!("/pool/agents/{TEST_UUID}.ready.json"));
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            Some(PathCategory::WorkerReady {
                id: TEST_UUID.to_string()
            })
        );
    }

    #[test]
    fn worker_ready_ignored_on_folder_events() {
        // Folder events don't trigger WorkerReady
        let path = PathBuf::from("/pool/agents/abc123.ready.json");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Worker response file
    // =========================================================================

    #[test]
    fn worker_response_on_file_written() {
        let path = PathBuf::from(format!("/pool/agents/{TEST_UUID}.response.json"));
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            Some(PathCategory::WorkerResponse {
                id: TEST_UUID.to_string()
            })
        );
    }

    #[test]
    fn worker_response_ignored_on_folder_events() {
        // Folder events don't trigger WorkerResponse
        let path = PathBuf::from("/pool/agents/abc123.response.json");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Files without known suffix are ignored
    // =========================================================================

    #[test]
    fn unknown_file_in_agents_not_categorized() {
        let path = PathBuf::from("/pool/agents/abc123.task.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn nested_files_under_agents_not_categorized() {
        // Subdirectories under agents are not categorized (flat structure)
        let path = PathBuf::from("/pool/agents/subdir/abc123.ready.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Submission request
    // =========================================================================

    #[test]
    fn submission_request_on_file_written() {
        let path = PathBuf::from("/pool/submissions/abc123.request.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            Some(PathCategory::SubmissionRequest {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_request_ignored_on_folder_events() {
        // Folder events should not trigger SubmissionRequest
        let path = PathBuf::from("/pool/submissions/abc123.request.json");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn submission_request_uuid_format() {
        let path = PathBuf::from(format!("/pool/submissions/{TEST_UUID}.request.json"));
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            Some(PathCategory::SubmissionRequest {
                id: TEST_UUID.to_string()
            })
        );
    }

    // =========================================================================
    // Submission response (daemon writes, we don't react)
    // =========================================================================

    #[test]
    fn submission_response_not_categorized() {
        // We don't need to react to our own response files
        let path = PathBuf::from("/pool/submissions/abc123.response.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn submission_other_file_not_categorized() {
        let path = PathBuf::from("/pool/submissions/abc123.metadata.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn submission_nested_file_not_categorized() {
        // Subdirectories under submissions are not categorized
        let path = PathBuf::from("/pool/submissions/abc123/task.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Unrelated paths
    // =========================================================================

    #[test]
    fn unrelated_path() {
        let path = PathBuf::from("/other/path");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn agents_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/agents");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn submissions_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/submissions");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn sibling_of_agents_not_categorized() {
        let path = PathBuf::from("/pool/logs/something");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Edge cases
    // =========================================================================

    #[test]
    fn relative_path_does_not_match_absolute() {
        let path = PathBuf::from("agents/abc123.ready.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn different_root_does_not_match() {
        let path = PathBuf::from("/other/pool/agents/abc123.ready.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }
}
