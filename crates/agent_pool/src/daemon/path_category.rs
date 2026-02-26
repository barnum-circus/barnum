//! Path categorization for filesystem events.
//!
//! Categorizes filesystem paths to determine what kind of entity they represent
//! (agent directory, response file, submission request, etc.).
//!
//! The categorization takes both path AND event kind into account, only returning
//! a category when the event is meaningful for that path type. This avoids race
//! conditions where we might try to read a file before it's fully written.

use std::path::Path;

use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, ModifyKind, RemoveKind};

use crate::constants::{REQUEST_SUFFIX, RESPONSE_FILE};

/// Category of a filesystem path.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PathCategory {
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
    /// Submission request file: `pending/<id>.request.json`
    SubmissionRequest {
        /// The submission's ID.
        id: String,
    },
}

/// Check if event kind indicates a file write is complete.
///
/// Accepts events from both Linux inotify and macOS `FSEvents`:
/// - `Close(Write)` - Linux inotify (file handle closed after write)
/// - `Create(File)` - macOS `FSEvents` (file created)
/// - `Modify(Data)` - macOS `FSEvents` (file content changed)
///
/// Atomic rename patterns are explicitly NOT supported - the protocol requires
/// direct writes.
const fn is_write_complete(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Access(AccessKind::Close(AccessMode::Write))
            | EventKind::Create(CreateKind::File)
            | EventKind::Modify(ModifyKind::Data(_))
    )
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
    pending_dir: &Path,
) -> Option<PathCategory> {
    categorize_under_agents(path, event_kind, agents_dir)
        .or_else(|| categorize_under_pending(path, event_kind, pending_dir))
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

    let name = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        // Agent directory - meaningful on folder creation or removal
        1 if is_folder_created(event_kind) || is_folder_removed(event_kind) => {
            Some(PathCategory::AgentDir { name })
        }
        // Agent response - only meaningful when write is complete
        2 if is_write_complete(event_kind) => {
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

fn categorize_under_pending(
    path: &Path,
    event_kind: EventKind,
    pending_dir: &Path,
) -> Option<PathCategory> {
    // Only process when write is complete
    if !is_write_complete(event_kind) {
        return None;
    }

    let relative = path.strip_prefix(pending_dir).ok()?;
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

    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind};

    use super::*;

    fn agents() -> PathBuf {
        PathBuf::from("/pool/agents")
    }

    fn pending() -> PathBuf {
        PathBuf::from("/pool/pending")
    }

    fn folder_created() -> EventKind {
        EventKind::Create(CreateKind::Folder)
    }

    fn write_complete() -> EventKind {
        EventKind::Access(AccessKind::Close(AccessMode::Write))
    }

    fn file_created() -> EventKind {
        EventKind::Create(CreateKind::File)
    }

    fn folder_removed() -> EventKind {
        EventKind::Remove(RemoveKind::Folder)
    }

    fn data_modified() -> EventKind {
        EventKind::Modify(ModifyKind::Data(DataChange::Content))
    }

    // =========================================================================
    // Agent directory
    // =========================================================================

    #[test]
    fn agent_directory_on_folder_create() {
        let path = PathBuf::from("/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_on_folder_remove() {
        let path = PathBuf::from("/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_removed(), &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_ignored_on_other_events() {
        let path = PathBuf::from("/pool/agents/claude-1");
        // File created event should not trigger AgentDir
        assert_eq!(
            categorize(&path, file_created(), &agents(), &pending()),
            None
        );
        // Write complete event should not trigger AgentDir
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn agent_directory_with_dots() {
        let path = PathBuf::from("/pool/agents/agent.v2.0");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "agent.v2.0".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_with_underscores() {
        let path = PathBuf::from("/pool/agents/my_agent_name");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "my_agent_name".to_string()
            })
        );
    }

    // =========================================================================
    // Agent response
    // =========================================================================

    #[test]
    fn agent_response_on_write_complete() {
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            Some(PathCategory::AgentResponse {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_response_on_file_created() {
        // macOS FSEvents sends Create(File) instead of Close(Write)
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        assert_eq!(
            categorize(&path, file_created(), &agents(), &pending()),
            Some(PathCategory::AgentResponse {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_response_on_data_modified() {
        // macOS FSEvents sends Modify(Data) for content changes
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        assert_eq!(
            categorize(&path, data_modified(), &agents(), &pending()),
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
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn agent_task_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/task.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn agent_other_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/debug.log");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn agent_nested_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/subdir/response.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    // =========================================================================
    // Submission request
    // =========================================================================

    #[test]
    fn submission_request_on_write_complete() {
        let path = PathBuf::from("/pool/pending/abc123.request.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_request_on_file_created() {
        // macOS FSEvents sends Create(File) instead of Close(Write)
        let path = PathBuf::from("/pool/pending/abc123.request.json");
        assert_eq!(
            categorize(&path, file_created(), &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_request_on_data_modified() {
        // macOS FSEvents sends Modify(Data) for content changes
        let path = PathBuf::from("/pool/pending/abc123.request.json");
        assert_eq!(
            categorize(&path, data_modified(), &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_request_ignored_on_folder_events() {
        let path = PathBuf::from("/pool/pending/abc123.request.json");
        // Folder events should not trigger SubmissionRequest
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn submission_request_uuid_format() {
        let path = PathBuf::from("/pool/pending/550e8400-e29b-41d4-a716-446655440000.request.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "550e8400-e29b-41d4-a716-446655440000".to_string()
            })
        );
    }

    // =========================================================================
    // Submission response (daemon writes, we don't react)
    // =========================================================================

    #[test]
    fn submission_response_not_categorized() {
        // We don't need to react to our own response files
        let path = PathBuf::from("/pool/pending/abc123.response.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn submission_other_file_not_categorized() {
        let path = PathBuf::from("/pool/pending/abc123.metadata.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn submission_nested_file_not_categorized() {
        // Subdirectories under pending are not categorized
        let path = PathBuf::from("/pool/pending/abc123/task.json");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn submission_directory_not_categorized() {
        // Plain directories under pending are not categorized (flat structure)
        let path = PathBuf::from("/pool/pending/abc123");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
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
            categorize(&path, write_complete(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn agents_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/agents");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn pending_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/pending");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn sibling_of_agents_not_categorized() {
        let path = PathBuf::from("/pool/logs/something");
        assert_eq!(
            categorize(&path, write_complete(), &agents(), &pending()),
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
            categorize(&path, folder_created(), &agents_dir, &pending()),
            None
        );
    }

    #[test]
    fn relative_path_does_not_match_absolute() {
        let path = PathBuf::from("agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }

    #[test]
    fn different_root_does_not_match() {
        let path = PathBuf::from("/other/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, folder_created(), &agents(), &pending()),
            None
        );
    }
}
