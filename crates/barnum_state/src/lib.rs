//! State persistence and resume for Barnum runs.
//!
//! Provides NDJSON state logging and reconstruction for crash recovery.
//! A state log records config + task events; on resume, replay the log
//! to reconstruct which tasks still need work.

mod reconstruct;
mod types;

pub use reconstruct::{
    ReconstructError, ReconstructedState, ReconstructedTask, WaitingTask, reconstruct,
};
pub use types::{
    FailureReason, FinallyRun, InvalidResponseReason, RetryOrigin, SpawnedOrigin, StateLogConfig,
    StateLogEntry, TaskCompleted, TaskFailed, TaskOrigin, TaskOutcome, TaskSubmitted, TaskSuccess,
};

use std::io::{self, BufRead, BufReader, Write};

/// Append a single state log entry as an NDJSON line.
///
/// Writes the JSON-serialized entry followed by a newline, then flushes.
/// Each call is atomic at the application level (one complete line).
///
/// # Errors
///
/// Returns an error if serialization or writing fails.
pub fn write_entry(writer: &mut impl Write, entry: &StateLogEntry) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, entry)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Read state log entries from an NDJSON source.
///
/// Returns an iterator yielding one `StateLogEntry` per line.
/// Empty trailing lines (from trailing newlines) are silently skipped.
///
/// # Errors
///
/// Each item in the iterator may be an `io::Error` from reading or parsing.
pub fn read_entries(reader: impl io::Read) -> impl Iterator<Item = io::Result<StateLogEntry>> {
    BufReader::new(reader)
        .lines()
        .filter_map(|line| match line {
            Ok(line) if line.is_empty() => None,
            Ok(line) => Some(
                serde_json::from_str(&line)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            ),
            Err(e) => Some(Err(e)),
        })
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use barnum_types::{LogTaskId, StepInputValue, StepName};
    use serde_json::json;
    use std::io::Cursor;

    fn sample_config_entry() -> StateLogEntry {
        StateLogEntry::Config(StateLogConfig {
            config: json!({"steps": [{"name": "Analyze"}]}),
        })
    }

    fn sample_submit_entry() -> StateLogEntry {
        StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(0),
            step: StepName::new("Analyze"),
            value: StepInputValue(json!({"url": "https://example.com"})),
            origin: TaskOrigin::Seed,
        })
    }

    fn sample_complete_entry() -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(0),
            outcome: TaskOutcome::Success(TaskSuccess {
                finally_value: StepInputValue(json!({"url": "https://example.com"})),
                children: vec![TaskSubmitted {
                    task_id: LogTaskId(1),
                    step: StepName::new("Process"),
                    value: StepInputValue(json!({"data": "x"})),
                    origin: TaskOrigin::Spawned(SpawnedOrigin {
                        parent_id: Some(LogTaskId(0)),
                    }),
                }],
            }),
        })
    }

    fn sample_finally_run_entry() -> StateLogEntry {
        StateLogEntry::FinallyRun(FinallyRun {
            finally_for: LogTaskId(0),
            children: vec![],
        })
    }

    // ==================== Write Tests ====================

    #[test]
    fn write_entry_appends_newline() {
        let mut buf = Vec::new();
        write_entry(&mut buf, &sample_config_entry()).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.ends_with('\n'));
        assert_eq!(output.matches('\n').count(), 1);
    }

    #[test]
    fn write_entry_produces_valid_json_per_line() {
        let mut buf = Vec::new();
        write_entry(&mut buf, &sample_config_entry()).unwrap();
        write_entry(&mut buf, &sample_submit_entry()).unwrap();
        write_entry(&mut buf, &sample_complete_entry()).unwrap();
        write_entry(&mut buf, &sample_finally_run_entry()).unwrap();

        let output = String::from_utf8(buf).unwrap();
        for line in output.lines() {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn write_entry_includes_kind_tag() {
        let mut buf = Vec::new();
        write_entry(&mut buf, &sample_config_entry()).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["kind"], "Config");
    }

    // ==================== Read Tests ====================

    #[test]
    fn read_entries_parses_ndjson() {
        let mut buf = Vec::new();
        write_entry(&mut buf, &sample_config_entry()).unwrap();
        write_entry(&mut buf, &sample_submit_entry()).unwrap();

        let entries: Vec<_> = read_entries(Cursor::new(buf))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], StateLogEntry::Config(_)));
        assert!(matches!(entries[1], StateLogEntry::TaskSubmitted(_)));
    }

    #[test]
    fn read_entries_handles_empty_input() {
        let entries: Vec<_> = read_entries(Cursor::new(Vec::new()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn read_entries_handles_trailing_newline() {
        let mut buf = Vec::new();
        write_entry(&mut buf, &sample_config_entry()).unwrap();
        // Extra trailing newline
        buf.extend_from_slice(b"\n");

        let entries: Vec<_> = read_entries(Cursor::new(buf))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn read_entries_errors_on_invalid_json() {
        let buf = b"not valid json\n";
        let results: Vec<_> = read_entries(Cursor::new(buf.to_vec())).collect();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    // ==================== Round-trip Tests ====================

    #[test]
    fn roundtrip_config_entry() {
        let mut buf = Vec::new();
        let entry = sample_config_entry();
        write_entry(&mut buf, &entry).unwrap();

        let entries: Vec<_> = read_entries(Cursor::new(buf))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        if let StateLogEntry::Config(c) = &entries[0] {
            assert_eq!(c.config, json!({"steps": [{"name": "Analyze"}]}));
        } else {
            unreachable!();
        }
    }

    #[test]
    fn roundtrip_all_entry_types() {
        let entries = vec![
            sample_config_entry(),
            sample_submit_entry(),
            sample_complete_entry(),
            StateLogEntry::TaskCompleted(TaskCompleted {
                task_id: LogTaskId(1),
                outcome: TaskOutcome::Failed(TaskFailed {
                    reason: FailureReason::Timeout,
                    retry: Some(TaskSubmitted {
                        task_id: LogTaskId(2),
                        step: StepName::new("Analyze"),
                        value: StepInputValue(json!(null)),
                        origin: TaskOrigin::Retry(RetryOrigin {
                            replaces: LogTaskId(1),
                        }),
                    }),
                }),
            }),
            sample_finally_run_entry(),
        ];

        let mut buf = Vec::new();
        for entry in &entries {
            write_entry(&mut buf, entry).unwrap();
        }

        let read_back: Vec<_> = read_entries(Cursor::new(buf))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(read_back.len(), entries.len());

        // Verify kinds match
        for (original, read) in entries.iter().zip(read_back.iter()) {
            match (original, read) {
                (StateLogEntry::Config(_), StateLogEntry::Config(_)) => {}
                (StateLogEntry::TaskSubmitted(a), StateLogEntry::TaskSubmitted(b)) => {
                    assert_eq!(a.task_id, b.task_id);
                    assert_eq!(a.step, b.step);
                    assert_eq!(a.origin, b.origin);
                }
                (StateLogEntry::TaskCompleted(a), StateLogEntry::TaskCompleted(b)) => {
                    assert_eq!(a.task_id, b.task_id);
                }
                (StateLogEntry::FinallyRun(a), StateLogEntry::FinallyRun(b)) => {
                    assert_eq!(a.finally_for, b.finally_for);
                    assert_eq!(a.children.len(), b.children.len());
                }
                _ => unreachable!("entry type mismatch"),
            }
        }
    }

    #[test]
    fn roundtrip_failure_reasons() {
        let reasons = [
            FailureReason::Timeout,
            FailureReason::AgentLost,
            FailureReason::InvalidResponse(InvalidResponseReason {
                message: "bad JSON".to_string(),
            }),
        ];

        for (i, reason) in reasons.iter().enumerate() {
            let entry = StateLogEntry::TaskCompleted(TaskCompleted {
                #[expect(clippy::cast_possible_truncation)]
                task_id: LogTaskId(i as u32),
                outcome: TaskOutcome::Failed(TaskFailed {
                    reason: reason.clone(),
                    retry: None,
                }),
            });

            let mut buf = Vec::new();
            write_entry(&mut buf, &entry).unwrap();

            let read_back: Vec<_> = read_entries(Cursor::new(buf))
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            if let StateLogEntry::TaskCompleted(c) = &read_back[0] {
                if let TaskOutcome::Failed(f) = &c.outcome {
                    assert_eq!(f.reason, *reason);
                } else {
                    unreachable!();
                }
            } else {
                unreachable!();
            }
        }
    }

    #[test]
    fn roundtrip_finally_run_with_children() {
        let entry = StateLogEntry::FinallyRun(FinallyRun {
            finally_for: LogTaskId(5),
            children: vec![TaskSubmitted {
                task_id: LogTaskId(10),
                step: StepName::new("Cleanup"),
                value: StepInputValue(json!({"target": "temp"})),
                origin: TaskOrigin::Spawned(SpawnedOrigin {
                    parent_id: Some(LogTaskId(5)),
                }),
            }],
        });

        let mut buf = Vec::new();
        write_entry(&mut buf, &entry).unwrap();

        let read_back: Vec<_> = read_entries(Cursor::new(buf))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(read_back.len(), 1);
        if let StateLogEntry::FinallyRun(f) = &read_back[0] {
            assert_eq!(f.finally_for, LogTaskId(5));
            assert_eq!(f.children.len(), 1);
            assert_eq!(f.children[0].task_id, LogTaskId(10));
            assert_eq!(f.children[0].step, "Cleanup");
        } else {
            unreachable!();
        }
    }
}
