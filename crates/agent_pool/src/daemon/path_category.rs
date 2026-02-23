//! Path categorization for filesystem events.
//!
//! Categorizes filesystem paths to determine what kind of entity they represent
//! (agent directory, response file, pending submission, etc.).

use std::path::Path;

use crate::constants::{RESPONSE_FILE, TASK_FILE};

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
    /// Pending submission directory: `pending/<uuid>/`
    PendingDir {
        /// The submission's UUID.
        uuid: String,
    },
    /// Pending submission task file: `pending/<uuid>/task.json`
    PendingTask {
        /// The submission's UUID.
        uuid: String,
    },
}

/// Categorize a filesystem path relative to the pool root.
///
/// Returns `None` if the path doesn't match any known category.
#[must_use]
pub(super) fn categorize(path: &Path, agents_dir: &Path, pending_dir: &Path) -> Option<PathCategory> {
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

    if components.is_empty() {
        return None;
    }

    let uuid = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::PendingDir { uuid }),
        2 => {
            let filename = components[1].as_os_str().to_str()?;
            if filename == TASK_FILE {
                Some(PathCategory::PendingTask { uuid })
            } else {
                None
            }
        }
        _ => None,
    }
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
    // Pending directory
    // =========================================================================

    #[test]
    fn pending_directory() {
        let path = PathBuf::from("/pool/pending/abc123");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::PendingDir {
                uuid: "abc123".to_string()
            })
        );
    }

    #[test]
    fn pending_directory_uuid_format() {
        let path = PathBuf::from("/pool/pending/550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::PendingDir {
                uuid: "550e8400-e29b-41d4-a716-446655440000".to_string()
            })
        );
    }

    // =========================================================================
    // Pending task
    // =========================================================================

    #[test]
    fn pending_task_file() {
        let path = PathBuf::from("/pool/pending/abc123/task.json");
        assert_eq!(
            categorize(&path, &agents(), &pending()),
            Some(PathCategory::PendingTask {
                uuid: "abc123".to_string()
            })
        );
    }

    #[test]
    fn pending_response_file_not_categorized() {
        // We write responses, we don't read them
        let path = PathBuf::from("/pool/pending/abc123/response.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn pending_other_file_not_categorized() {
        let path = PathBuf::from("/pool/pending/abc123/metadata.json");
        assert_eq!(categorize(&path, &agents(), &pending()), None);
    }

    #[test]
    fn pending_nested_file_not_categorized() {
        let path = PathBuf::from("/pool/pending/abc123/subdir/task.json");
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
