//! Path categorization for filesystem events.
//!
//! Categorizes filesystem paths to determine what kind of entity they represent
//! (agent directory, response file, submission request, etc.).
//!
//! The categorization takes both path AND event kind into account, only returning
//! a category when the event is meaningful for that path type. This avoids race
//! conditions where we might try to read a file before it's fully written.

use std::path::Path;

use notify::event::{CreateKind, EventKind, RemoveKind};

use crate::constants::{READY_SUFFIX, REQUEST_SUFFIX, RESPONSE_FILE, WORKER_RESPONSE_SUFFIX};
use crate::fs::is_write_complete;

/// Category of a filesystem path.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PathCategory {
    // -------------------------------------------------------------------------
    // Legacy agent protocol (per-agent directories)
    // TODO: Remove these after anonymous workers migration is complete
    // -------------------------------------------------------------------------
    /// Agent directory: `agents/<name>/`
    AgentDir {
        /// The agent's directory name.
        name: String,
    },
    /// Agent response file: `agents/<name>/response.json`
    AgentResponse {
        /// The agent's directory name.
        name: String,
    },

    // -------------------------------------------------------------------------
    // Anonymous worker protocol (flat files)
    // -------------------------------------------------------------------------
    /// Worker ready file: `agents/<uuid>.ready.json`
    #[allow(dead_code)] // Not yet used until anonymous workers refactor
    WorkerReady {
        /// The worker's UUID.
        id: String,
    },
    /// Worker response file: `agents/<uuid>.response.json`
    #[allow(dead_code)] // Not yet used until anonymous workers refactor
    WorkerResponse {
        /// The worker's UUID.
        id: String,
    },

    // -------------------------------------------------------------------------
    // Submission protocol (unchanged)
    // -------------------------------------------------------------------------
    /// Submission request file: `submissions/<id>.request.json`
    SubmissionRequest {
        /// The submission's ID.
        id: String,
    },
}

/// Check if event kind indicates a folder was created.
const fn is_folder_created(kind: EventKind) -> bool {
    matches!(kind, EventKind::Create(CreateKind::Folder))
}

/// Check if event kind indicates a folder was removed.
const fn is_folder_removed(kind: EventKind) -> bool {
    matches!(kind, EventKind::Remove(RemoveKind::Folder))
}

/// Categorize a filesystem event (path + event kind).
///
/// Returns `Some(category)` only when the event is meaningful for that path type:
/// - `AgentDir`: only on folder creation (agent registering)
/// - `AgentResponse`: only on write complete (response ready to read)
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
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    match components.len() {
        // Single component: either a folder (legacy) or a flat file (anonymous workers)
        1 => {
            let name = components[0].as_os_str().to_str()?;

            // Legacy: Agent directory - meaningful on folder creation or removal
            if is_folder_created(event_kind) || is_folder_removed(event_kind) {
                return Some(PathCategory::AgentDir {
                    name: name.to_string(),
                });
            }

            // Anonymous workers: flat files - only meaningful when write is complete
            if is_write_complete(event_kind) {
                if let Some(id) = name.strip_suffix(READY_SUFFIX) {
                    return Some(PathCategory::WorkerReady { id: id.to_string() });
                }
                if let Some(id) = name.strip_suffix(WORKER_RESPONSE_SUFFIX) {
                    return Some(PathCategory::WorkerResponse { id: id.to_string() });
                }
            }

            None
        }
        // Two components: legacy agent response file `<name>/response.json`
        2 if is_write_complete(event_kind) => {
            let name = components[0].as_os_str().to_str()?.to_string();
            let filename = components[1].as_os_str().to_str()?;
            if filename == RESPONSE_FILE {
                Some(PathCategory::AgentResponse { name })
            } else {
                None
            }
        }
        _ => None,
    }
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

    use notify::event::{CreateKind, RemoveKind};

    use super::*;

    const TEST_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn agents() -> PathBuf {
        PathBuf::from("/pool/agents")
    }

    fn submissions() -> PathBuf {
        PathBuf::from("/pool/submissions")
    }

    fn folder_created() -> EventKind {
        EventKind::Create(CreateKind::Folder)
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

    fn folder_removed() -> EventKind {
        EventKind::Remove(RemoveKind::Folder)
    }

    // =========================================================================
    // Agent directory
    // =========================================================================

    #[test]
    fn agent_directory_on_folder_create() {
        let path = PathBuf::from("/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_on_folder_remove() {
        let path = PathBuf::from("/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_removed(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_ignored_on_file_events() {
        let path = PathBuf::from("/pool/agents/claude-1");
        // File write events should not trigger AgentDir (only folder events)
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn agent_directory_with_dots() {
        let path = PathBuf::from("/pool/agents/agent.v2.0");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "agent.v2.0".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_with_underscores() {
        let path = PathBuf::from("/pool/agents/my_agent_name");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "my_agent_name".to_string()
            })
        );
    }

    // =========================================================================
    // Agent response
    // =========================================================================

    #[test]
    fn agent_response_on_file_written() {
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            Some(PathCategory::AgentResponse {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_response_ignored_on_folder_events() {
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        // Folder events should not trigger AgentResponse
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn agent_task_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/task.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn agent_other_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/debug.log");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn agent_nested_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/subdir/response.json");
        assert_eq!(
            categorize(&path, file_written(), &agents(), &submissions()),
            None
        );
    }

    // =========================================================================
    // Anonymous worker: ready file
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
    fn worker_ready_folder_treated_as_agent_dir() {
        // Edge case: if a folder named "abc123.ready.json" is created, it's
        // treated as a legacy AgentDir (not WorkerReady). In practice this
        // won't happen - workers create files, not folders.
        let path = PathBuf::from("/pool/agents/abc123.ready.json");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "abc123.ready.json".to_string()
            })
        );
    }

    // =========================================================================
    // Anonymous worker: response file
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
    fn worker_response_folder_treated_as_agent_dir() {
        // Edge case: if a folder named "abc123.response.json" is created, it's
        // treated as a legacy AgentDir (not WorkerResponse). In practice this
        // won't happen - workers create files, not folders.
        let path = PathBuf::from("/pool/agents/abc123.response.json");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            Some(PathCategory::AgentDir {
                name: "abc123.response.json".to_string()
            })
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
        let path = PathBuf::from("/pool/submissions/abc123.request.json");
        // Folder events should not trigger SubmissionRequest
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

    #[test]
    fn submission_directory_not_categorized() {
        // Plain directories under submissions are not categorized (flat structure)
        let path = PathBuf::from("/pool/submissions/abc123");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
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
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn submissions_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/submissions");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
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
    fn empty_agent_name_still_categorized() {
        // Filesystem allows empty names in theory, we just pass through
        let agents_dir = PathBuf::from("/pool/agents/");
        let path = PathBuf::from("/pool/agents//");
        // This won't match because empty component
        assert_eq!(
            categorize(&path, folder_created(), &agents_dir, &submissions()),
            None
        );
    }

    #[test]
    fn relative_path_does_not_match_absolute() {
        let path = PathBuf::from("agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }

    #[test]
    fn different_root_does_not_match() {
        let path = PathBuf::from("/other/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &submissions()),
            None
        );
    }
}
