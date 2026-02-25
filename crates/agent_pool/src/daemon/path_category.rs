//! Path categorization for filesystem events.
//!
//! Categorizes filesystem paths to determine what kind of entity they represent
//! (agent directory, response file, submission request, etc.).

use std::path::Path;

use crate::constants::{REQUEST_SUFFIX, RESPONSE_FILE, RESPONSE_SUFFIX};

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
    /// Submission response file: `pending/<id>.response.json` (daemon writes, ignored)
    SubmissionResponse {
        /// The submission's ID.
        id: String,
    },
}

/// Categorize a filesystem path relative to the pool root.
///
/// Returns `None` if the path doesn't match any known category.
#[must_use]
pub(super) fn categorize(
    path: &Path,
    agents_dir: &Path,
    pending_dir: &Path,
) -> Option<PathCategory> {
    categorize_under_agents(path, agents_dir)
        .or_else(|| categorize_under_pending(path, pending_dir))
}

fn categorize_under_agents(path: &Path, agents_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    let name = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::AgentDir { name }),
        2 => {
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

fn categorize_under_pending(path: &Path, pending_dir: &Path) -> Option<PathCategory> {
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

    if let Some(id) = filename.strip_suffix(RESPONSE_SUFFIX) {
        return Some(PathCategory::SubmissionResponse { id: id.to_string() });
    }

    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn agents() -> PathBuf {
        PathBuf::from("/pool/agents")
    }

    fn pending() -> PathBuf {
        PathBuf::from("/pool/pending")
    }

    // =========================================================================
    // Agent directory
    // =========================================================================

    #[test]
    fn agent_directory() {
        let path = PathBuf::from("/pool/agents/claude-1");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_with_dots() {
        let path = PathBuf::from("/pool/agents/agent.v2.0");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "agent.v2.0".to_string()
            })
        );
    }

    #[test]
    fn agent_directory_with_underscores() {
        let path = PathBuf::from("/pool/agents/my_agent_name");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::AgentDir {
                name: "my_agent_name".to_string()
            })
        );
    }

    // =========================================================================
    // Agent response
    // =========================================================================

    #[test]
    fn agent_response_file() {
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::AgentResponse {
                name: "claude-1".to_string()
            })
        );
    }

    #[test]
    fn agent_task_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/task.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn agent_other_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/debug.log");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn agent_nested_file_not_categorized() {
        let path = PathBuf::from("/pool/agents/claude-1/subdir/response.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    // =========================================================================
    // Submission request
    // =========================================================================

    #[test]
    fn submission_request_file() {
        let path = PathBuf::from("/pool/pending/abc123.request.json");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_request_uuid_format() {
        let path = PathBuf::from("/pool/pending/550e8400-e29b-41d4-a716-446655440000.request.json");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::SubmissionRequest {
                id: "550e8400-e29b-41d4-a716-446655440000".to_string()
            })
        );
    }

    // =========================================================================
    // Submission response
    // =========================================================================

    #[test]
    fn submission_response_file() {
        let path = PathBuf::from("/pool/pending/abc123.response.json");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::SubmissionResponse {
                id: "abc123".to_string()
            })
        );
    }

    #[test]
    fn submission_other_file_not_categorized() {
        let path = PathBuf::from("/pool/pending/abc123.metadata.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn submission_nested_file_not_categorized() {
        // Subdirectories under pending are not categorized
        let path = PathBuf::from("/pool/pending/abc123/task.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn submission_directory_not_categorized() {
        // Plain directories under pending are not categorized (flat structure)
        let path = PathBuf::from("/pool/pending/abc123");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    // =========================================================================
    // Unrelated paths
    // =========================================================================

    #[test]
    fn unrelated_path() {
        let path = PathBuf::from("/other/path");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn agents_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/agents");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn pending_dir_itself_not_categorized() {
        let path = PathBuf::from("/pool/pending");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn sibling_of_agents_not_categorized() {
        let path = PathBuf::from("/pool/logs/something");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
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
        assert_eq!(categorize(&path, &agents_dir, &pending()), None);
    }

    #[test]
    fn relative_path_does_not_match_absolute() {
        let path = PathBuf::from("agents/claude-1");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn different_root_does_not_match() {
        let path = PathBuf::from("/other/pool/agents/claude-1");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }
}
