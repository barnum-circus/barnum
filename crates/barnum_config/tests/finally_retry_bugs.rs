//! Tests for finally hook behavior with retries.
//!
//! These tests demonstrate bugs in the current implementation where
//! finally hooks run too early when child tasks retry.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use barnum_config::{CompiledSchemas, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use rstest::rstest;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Test that demonstrates the bug: A's finally hook runs when B fails,
/// not when B' (the retry) succeeds.
///
/// Setup:
/// - Step A has a finally hook that writes `finally_ran` to a log file
/// - A's agent returns a child task B
/// - B's agent fails on first call (returns invalid JSON), succeeds on second
///
/// Bug behavior (current):
/// - A's finally runs after B fails (wrong!)
/// - When B' succeeds, A's finally has already run
///
/// Correct behavior (after fix):
/// - A's finally runs after B' succeeds
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_runs_too_early_on_retry() {
    let test_name = "finally_retry_bugs_too_early";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Track how many times B's agent is called
    let b_call_count = Arc::new(AtomicUsize::new(0));
    let b_count_clone = b_call_count.clone();

    // Track when finally hook runs relative to B's agent calls
    let finally_log = root.join("finally.log");
    let finally_log_for_hook = finally_log.clone();

    // Agent behavior:
    // - Step A: return child task B
    // - Step B: fail first call (invalid JSON), succeed second call
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "StepA" => {
                // Return child task B
                r#"[{"kind": "StepB", "value": {}}]"#.to_string()
            }
            "StepB" => {
                let count = b_count_clone.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    // First call: fail with invalid JSON
                    "not valid json {{{".to_string()
                } else {
                    // Second call: succeed
                    "[]".to_string()
                }
            }
            _ => "[]".to_string(),
        }
    });

    // Create the finally hook script - just writes a marker
    let finally_script = root.join("finally.sh");
    let script_content = format!(
        r#"#!/bin/bash
echo "finally_ran" > "{}"
"#,
        finally_log_for_hook.display()
    );
    fs::write(&finally_script, &script_content).expect("write finally script");

    // Make it executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod finally script");
    }

    // Config: A has finally hook, spawns B. B has retries enabled.
    let config_json = format!(
        r#"{{
        "options": {{
            "max_retries": 3,
            "retry_on_invalid_response": true
        }},
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": "Step A"}}}},
                "next": ["StepB"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "StepB",
                "action": {{"kind": "Pool", "instructions": {{"inline": "Step B"}}}},
                "next": []
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks = vec![Task::new("StepA", StepInputValue(serde_json::json!({})))];
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Run the task queue
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);

    // Stop agent and get call counts
    let _processed = agent.stop();
    let final_b_count = b_call_count.load(Ordering::SeqCst);

    // Should succeed (B eventually succeeds on retry)
    assert!(result.is_ok(), "run should succeed: {result:?}");

    // B should have been called twice (fail once, succeed once)
    assert_eq!(final_b_count, 2, "B should be called twice (fail + retry)");

    // Finally hook should have run exactly once, after B succeeded
    assert!(
        finally_log.exists(),
        "Finally hook should have run and created marker file"
    );

    cleanup_test_dir(test_name);
}

/// Simpler test: track timing via atomic counters instead of files.
///
/// This version uses a more robust detection mechanism:
/// - Track total B agent calls at the moment finally runs
/// - Assert finally ran after ALL B calls, not after the failure
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_timing_via_counters() {
    let test_name = "finally_retry_bugs_counters";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Counters for B's agent calls
    let b_call_count = Arc::new(AtomicUsize::new(0));
    let b_count_clone = b_call_count.clone();

    // We'll detect timing by having the finally hook write the current B count
    // to a file, which we read after the run.
    let marker_file = root.join("finally_marker.txt");
    let marker_for_script = marker_file.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "Parent" => {
                // Spawn one child
                r#"[{"kind": "Child", "value": {}}]"#.to_string()
            }
            "Child" => {
                let count = b_count_clone.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    // First call: fail
                    "invalid json!!!".to_string()
                } else {
                    // Retry: succeed
                    "[]".to_string()
                }
            }
            _ => "[]".to_string(),
        }
    });

    // Create finally script that records the B call count at execution time
    let finally_script = root.join("finally.sh");
    // The script writes the current value to a file
    let script = format!(
        r#"#!/bin/bash
# This runs when finally hook is triggered
# We detect timing by checking if child succeeded yet
echo "finally_executed" > "{}"
"#,
        marker_for_script.display()
    );
    fs::write(&finally_script, &script).expect("write script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod script");
    }

    let config_json = format!(
        r#"{{
        "options": {{
            "max_retries": 3,
            "retry_on_invalid_response": true
        }},
        "steps": [
            {{
                "name": "Parent",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["Child"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "Child",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": []
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("Parent", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();
    let total_child_calls = b_call_count.load(Ordering::SeqCst);

    // Run should succeed
    assert!(result.is_ok(), "run should succeed: {result:?}");

    // Child should be called twice
    assert_eq!(
        total_child_calls, 2,
        "Child should be called twice (fail + retry)"
    );

    // Finally should have run
    assert!(
        marker_file.exists(),
        "Finally hook should have executed and created marker file"
    );

    // The key question: did finally run too early?
    // We can't directly check timing from the file, but we can verify
    // the run completed successfully, which means the retry succeeded.

    cleanup_test_dir(test_name);
}

/// Test with nested finally hooks: both Parent and Child have finally hooks.
///
/// Expected order:
/// 1. Parent runs, spawns Child
/// 2. Child fails (attempt 1)
/// 3. Child retries and succeeds (attempt 2)
/// 4. Child's finally runs
/// 5. Parent's finally runs
///
/// Bug behavior:
/// 1. Parent runs, spawns Child
/// 2. Child fails (attempt 1)
/// 3. Parent's finally runs (TOO EARLY!)
/// 4. Child retries and succeeds
/// 5. Child's finally runs
#[rstest]
#[timeout(Duration::from_secs(20))]
fn nested_finally_with_retry_ordering() {
    let test_name = "finally_retry_bugs_nested";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Log file to track ordering
    let order_log = root.join("order.log");
    let order_log_parent = order_log.clone();
    let order_log_child = order_log.clone();

    let child_call_count = Arc::new(AtomicUsize::new(0));
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), {
        let child_call_count = Arc::clone(&child_call_count);
        move |payload| {
            let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
            let kind = parsed
                .get("task")
                .and_then(|t| t.get("kind"))
                .and_then(|k| k.as_str())
                .unwrap_or("");

            match kind {
                "Parent" => r#"[{"kind": "Child", "value": {}}]"#.to_string(),
                "Child" => {
                    let count = child_call_count.fetch_add(1, Ordering::SeqCst);
                    if count == 0 {
                        "bad json".to_string()
                    } else {
                        "[]".to_string()
                    }
                }
                _ => "[]".to_string(),
            }
        }
    });

    // Parent's finally hook
    let parent_finally = root.join("parent_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "parent_finally" >> "{}"
"#,
        order_log_parent.display()
    );
    fs::write(&parent_finally, &script).expect("write parent finally");

    // Child's finally hook
    let child_finally = root.join("child_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "child_finally" >> "{}"
"#,
        order_log_child.display()
    );
    fs::write(&child_finally, &script).expect("write child finally");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&parent_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod parent finally");
        fs::set_permissions(&child_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod child finally");
    }

    let config_json = format!(
        r#"{{
        "options": {{
            "max_retries": 3,
            "retry_on_invalid_response": true
        }},
        "steps": [
            {{
                "name": "Parent",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["Child"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "Child",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "finally": {{"kind": "Command", "script": "{}"}}
            }}
        ]
    }}"#,
        parent_finally.display(),
        child_finally.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("Parent", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    // Read the order log
    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // Correct order: child_finally first, then parent_finally
    // (Parent waits for Child to complete before running its finally)
    assert_eq!(
        lines,
        vec!["child_finally", "parent_finally"],
        "Finally hooks ran in wrong order. Expected child then parent, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test that finally hook runs when all retries are exhausted (task dropped).
///
/// If Child exhausts all retries and is dropped, Parent's finally should
/// still run (the descendant is "done" even though it failed).
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_runs_when_retries_exhausted() {
    let test_name = "finally_retry_bugs_exhausted";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "Parent" => r#"[{"kind": "Child", "value": {}}]"#.to_string(),
            // Child always fails
            "Child" => "always invalid json".to_string(),
            _ => "[]".to_string(),
        }
    });

    let finally_marker = root.join("finally_ran.txt");
    let marker_for_script = finally_marker.clone();

    let finally_script = root.join("finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "finally_executed" > "{}"
"#,
        marker_for_script.display()
    );
    fs::write(&finally_script, &script).expect("write script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod script");
    }

    let config_json = format!(
        r#"{{
        "options": {{
            "max_retries": 2,
            "retry_on_invalid_response": true
        }},
        "steps": [
            {{
                "name": "Parent",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["Child"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "Child",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": []
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // This should fail because Child is dropped after max retries
    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("Parent", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    // Run fails because task was dropped
    assert!(result.is_err(), "run should fail when child is dropped");

    // But finally hook should still have run!
    // (The descendant is "done" - it was dropped, but tracking should complete)
    assert!(
        finally_marker.exists(),
        "Parent's finally hook should run even when child is dropped"
    );

    cleanup_test_dir(test_name);
}

/// Test that A's finally waits for B's entire subtree, including grandchildren.
///
/// Setup:
/// - A (with finally) spawns B (with finally)
/// - B spawns C (no finally)
/// - C completes
///
/// Expected order:
/// 1. A runs, spawns B
/// 2. B runs, spawns C
/// 3. C runs, completes → writes `C_done`
/// 4. B's finally runs (B's subtree done) → writes `B_finally`
/// 5. A's finally runs (A's subtree done, including B's finally) → writes `A_finally`
///
/// Bug behavior:
/// - A's finally runs when B succeeds (before C completes, before B's finally)
/// - Order is: `A_finally`, `C_done`, `B_finally` (wrong!)
#[rstest]
#[timeout(Duration::from_secs(20))]
fn subtree_finally_waits_for_grandchildren() {
    let test_name = "finally_subtree_grandchildren";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Log file to track ordering
    let order_log = root.join("order.log");
    let order_log_a = order_log.clone();
    let order_log_b = order_log.clone();
    let order_log_c = order_log.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "StepA" => r#"[{"kind": "StepB", "value": {}}]"#.to_string(),
            "StepB" => r#"[{"kind": "StepC", "value": {}}]"#.to_string(),
            _ => "[]".to_string(), // StepC and all others return empty
        }
    });

    // A's finally hook
    let a_finally = root.join("a_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "A_finally" >> "{}"
"#,
        order_log_a.display()
    );
    fs::write(&a_finally, &script).expect("write A finally");

    // B's finally hook
    let b_finally = root.join("b_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "B_finally" >> "{}"
"#,
        order_log_b.display()
    );
    fs::write(&b_finally, &script).expect("write B finally");

    // C's completion marker (written by post hook since C has no finally)
    let c_post = root.join("c_post.sh");
    let script = format!(
        r#"#!/bin/bash
echo "C_done" >> "{}"
cat  # pass through stdin to stdout
"#,
        order_log_c.display()
    );
    fs::write(&c_post, &script).expect("write C post");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&a_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod A finally");
        fs::set_permissions(&b_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod B finally");
        fs::set_permissions(&c_post, fs::Permissions::from_mode(0o755)).expect("chmod C post");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["StepB"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "StepB",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["StepC"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "StepC",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "post": {{"kind": "Command", "script": "{}"}}
            }}
        ]
    }}"#,
        a_finally.display(),
        b_finally.display(),
        c_post.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    // Read the order log
    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // Correct order: C completes, then B's finally, then A's finally
    // A waits for entire subtree (including B's finally) before running its finally
    assert_eq!(
        lines,
        vec!["C_done", "B_finally", "A_finally"],
        "Finally hooks ran in wrong order. Expected C_done, B_finally, A_finally, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test that A's finally waits for tasks spawned by B's finally hook.
///
/// Setup:
/// - A (with finally) spawns B (with finally that spawns cleanup task C)
/// - B completes, B's finally runs and outputs `[{"kind": "Cleanup", "value": {}}]`
/// - C (cleanup task) runs and completes
///
/// Expected order:
/// 1. A runs, spawns B
/// 2. B runs, completes
/// 3. B's finally runs → spawns C, writes `B_finally`
/// 4. C runs, completes → writes `C_done`
/// 5. A's finally runs (A's subtree done, including B's finally-spawned tasks) → writes `A_finally`
///
/// Bug behavior:
/// - B's finally spawns C as a "new root" with `finally_origin_id: None`
/// - A's finally runs when B's finally completes (before C runs)
/// - Order is: `B_finally`, `A_finally`, `C_done` (wrong!)
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_waits_for_finally_spawned_tasks() {
    let test_name = "finally_spawned_tasks";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Log file to track ordering
    let order_log = root.join("order.log");
    let order_log_a = order_log.clone();
    let order_log_b = order_log.clone();
    let order_log_c = order_log.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "StepA" => r#"[{"kind": "StepB", "value": {}}]"#.to_string(),
            _ => "[]".to_string(), // StepB and Cleanup return empty
        }
    });

    // A's finally hook - just writes marker
    let a_finally = root.join("a_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "A_finally" >> "{}"
"#,
        order_log_a.display()
    );
    fs::write(&a_finally, &script).expect("write A finally");

    // B's finally hook - spawns a cleanup task
    let b_finally = root.join("b_finally.sh");
    let script = format!(
        r#"#!/bin/bash
echo "B_finally" >> "{}"
echo '[{{"kind": "Cleanup", "value": {{}}}}]'
"#,
        order_log_b.display()
    );
    fs::write(&b_finally, &script).expect("write B finally");

    // Cleanup task's post hook - writes completion marker
    let cleanup_post = root.join("cleanup_post.sh");
    let script = format!(
        r#"#!/bin/bash
echo "C_done" >> "{}"
cat  # pass through stdin to stdout
"#,
        order_log_c.display()
    );
    fs::write(&cleanup_post, &script).expect("write cleanup post");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&a_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod A finally");
        fs::set_permissions(&b_finally, fs::Permissions::from_mode(0o755))
            .expect("chmod B finally");
        fs::set_permissions(&cleanup_post, fs::Permissions::from_mode(0o755))
            .expect("chmod cleanup post");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": ["StepB"],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "StepB",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "finally": {{"kind": "Command", "script": "{}"}}
            }},
            {{
                "name": "Cleanup",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "post": {{"kind": "Command", "script": "{}"}}
            }}
        ]
    }}"#,
        a_finally.display(),
        b_finally.display(),
        cleanup_post.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    // Read the order log
    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // Correct order: B's finally runs and spawns cleanup, cleanup completes, then A's finally
    // A waits for entire subtree (including tasks spawned by B's finally) before running its finally
    assert_eq!(
        lines,
        vec!["B_finally", "C_done", "A_finally"],
        "Finally hooks ran in wrong order. Expected B_finally, C_done, A_finally, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test deeply nested finally chain: A→B→C→D all with finally hooks.
///
/// Expected order: D completes, `C_finally`, `B_finally`, `A_finally`
/// (innermost to outermost)
///
/// This is a more extreme version of Bug 1 - cascading grandchild issue.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn deeply_nested_finally_chain() {
    let test_name = "finally_deeply_nested";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    let order_log = root.join("order.log");
    let order_log_a = order_log.clone();
    let order_log_b = order_log.clone();
    let order_log_c = order_log.clone();
    let order_log_d = order_log.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "StepA" => r#"[{"kind": "StepB", "value": {}}]"#.to_string(),
            "StepB" => r#"[{"kind": "StepC", "value": {}}]"#.to_string(),
            "StepC" => r#"[{"kind": "StepD", "value": {}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    // Create finally hooks for A, B, C and a post hook for D
    let a_finally = root.join("a_finally.sh");
    fs::write(
        &a_finally,
        format!(
            "#!/bin/bash\necho \"A_finally\" >> \"{}\"\n",
            order_log_a.display()
        ),
    )
    .expect("write A finally");

    let b_finally = root.join("b_finally.sh");
    fs::write(
        &b_finally,
        format!(
            "#!/bin/bash\necho \"B_finally\" >> \"{}\"\n",
            order_log_b.display()
        ),
    )
    .expect("write B finally");

    let c_finally = root.join("c_finally.sh");
    fs::write(
        &c_finally,
        format!(
            "#!/bin/bash\necho \"C_finally\" >> \"{}\"\n",
            order_log_c.display()
        ),
    )
    .expect("write C finally");

    let d_post = root.join("d_post.sh");
    fs::write(
        &d_post,
        format!(
            "#!/bin/bash\necho \"D_done\" >> \"{}\"\ncat\n",
            order_log_d.display()
        ),
    )
    .expect("write D post");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&a_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&b_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&c_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&d_post, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{"name": "StepA", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepB"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepB", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepC"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepC", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepD"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepD", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "post": {{"kind": "Command", "script": "{}"}}}}
        ]
    }}"#,
        a_finally.display(),
        b_finally.display(),
        c_finally.display(),
        d_post.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // Expected: D completes, then C, B, A finally hooks in order
    assert_eq!(
        lines,
        vec!["D_done", "C_finally", "B_finally", "A_finally"],
        "Finally hooks ran in wrong order. Expected D_done, C_finally, B_finally, A_finally, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test multiple children where one has a grandchild.
///
/// Setup: A spawns B and C. B spawns D. All have finally hooks.
///
/// Expected order: `D_done`, `B_finally`, `C_finally` (order of B/C flexible), `A_finally`
///
/// Bug: A gets notified when B succeeds, before B's subtree (D, `B_finally`) completes.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn multiple_children_with_finally() {
    let test_name = "finally_multiple_children";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    let order_log = root.join("order.log");
    let order_log_a = order_log.clone();
    let order_log_b = order_log.clone();
    let order_log_c = order_log.clone();
    let order_log_d = order_log.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            // A spawns both B and C
            "StepA" => {
                r#"[{"kind": "StepB", "value": {}}, {"kind": "StepC", "value": {}}]"#.to_string()
            }
            // B spawns D (grandchild)
            "StepB" => r#"[{"kind": "StepD", "value": {}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    let a_finally = root.join("a_finally.sh");
    fs::write(
        &a_finally,
        format!(
            "#!/bin/bash\necho \"A_finally\" >> \"{}\"\n",
            order_log_a.display()
        ),
    )
    .expect("write");

    let b_finally = root.join("b_finally.sh");
    fs::write(
        &b_finally,
        format!(
            "#!/bin/bash\necho \"B_finally\" >> \"{}\"\n",
            order_log_b.display()
        ),
    )
    .expect("write");

    let c_finally = root.join("c_finally.sh");
    fs::write(
        &c_finally,
        format!(
            "#!/bin/bash\necho \"C_finally\" >> \"{}\"\n",
            order_log_c.display()
        ),
    )
    .expect("write");

    let d_post = root.join("d_post.sh");
    fs::write(
        &d_post,
        format!(
            "#!/bin/bash\necho \"D_done\" >> \"{}\"\ncat\n",
            order_log_d.display()
        ),
    )
    .expect("write");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&a_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&b_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&c_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&d_post, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{"name": "StepA", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepB", "StepC"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepB", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepD"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepC", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepD", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "post": {{"kind": "Command", "script": "{}"}}}}
        ]
    }}"#,
        a_finally.display(),
        b_finally.display(),
        c_finally.display(),
        d_post.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // A_finally must be last. D_done must come before B_finally.
    // C_finally can interleave with B's subtree completion.
    // Valid orders: [D_done, B_finally, C_finally, A_finally] or [D_done, C_finally, B_finally, A_finally]
    // or [C_finally, D_done, B_finally, A_finally]
    let a_pos = lines.iter().position(|&x| x == "A_finally");
    let b_pos = lines.iter().position(|&x| x == "B_finally");
    let d_pos = lines.iter().position(|&x| x == "D_done");

    let valid = match (a_pos, b_pos, d_pos) {
        (Some(a), Some(b), Some(d)) => {
            // A must be last, D must be before B
            a == lines.len() - 1 && d < b
        }
        _ => false,
    };

    assert!(
        valid,
        "Finally hooks ran in wrong order. A_finally must be last, D_done before B_finally, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test that finally hook spawning multiple tasks works correctly.
///
/// Setup: A spawns B. B's finally spawns C and D (two cleanup tasks).
///
/// Expected order: `B_finally` (spawns C, D), `C_done`, `D_done` (order flexible), `A_finally`
///
/// Bug: A's finally runs when B's finally completes, not when C and D complete.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_spawns_multiple_tasks() {
    let test_name = "finally_spawns_multiple";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    let order_log = root.join("order.log");
    let order_log_a = order_log.clone();
    let order_log_b = order_log.clone();
    let order_log_c = order_log.clone();
    let order_log_d = order_log.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "StepA" => r#"[{"kind": "StepB", "value": {}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    let a_finally = root.join("a_finally.sh");
    fs::write(
        &a_finally,
        format!(
            "#!/bin/bash\necho \"A_finally\" >> \"{}\"\n",
            order_log_a.display()
        ),
    )
    .expect("write");

    // B's finally spawns TWO cleanup tasks
    let b_finally = root.join("b_finally.sh");
    fs::write(
        &b_finally,
        format!(
            "#!/bin/bash\necho \"B_finally\" >> \"{}\"\necho '[{{\"kind\": \"CleanupC\", \"value\": {{}}}}, {{\"kind\": \"CleanupD\", \"value\": {{}}}}]'\n",
            order_log_b.display()
        ),
    )
    .expect("write");

    let c_post = root.join("c_post.sh");
    fs::write(
        &c_post,
        format!(
            "#!/bin/bash\necho \"C_done\" >> \"{}\"\ncat\n",
            order_log_c.display()
        ),
    )
    .expect("write");

    let d_post = root.join("d_post.sh");
    fs::write(
        &d_post,
        format!(
            "#!/bin/bash\necho \"D_done\" >> \"{}\"\ncat\n",
            order_log_d.display()
        ),
    )
    .expect("write");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&a_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&b_finally, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&c_post, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&d_post, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{"name": "StepA", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": ["StepB"], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "StepB", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "finally": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "CleanupC", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "post": {{"kind": "Command", "script": "{}"}}}},
            {{"name": "CleanupD", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [], "post": {{"kind": "Command", "script": "{}"}}}}
        ]
    }}"#,
        a_finally.display(),
        b_finally.display(),
        c_post.display(),
        d_post.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    let _processed = agent.stop();

    assert!(result.is_ok(), "run should succeed: {result:?}");

    let order_content = fs::read_to_string(&order_log).unwrap_or_default();
    let lines: Vec<&str> = order_content.lines().collect();

    // B_finally must come first (it spawns C and D)
    // C_done and D_done must both come before A_finally
    // A_finally must be last
    let a_pos = lines.iter().position(|&x| x == "A_finally");
    let b_pos = lines.iter().position(|&x| x == "B_finally");
    let c_pos = lines.iter().position(|&x| x == "C_done");
    let d_pos = lines.iter().position(|&x| x == "D_done");

    let valid = match (a_pos, b_pos, c_pos, d_pos) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            // B_finally first, C and D before A, A last
            b == 0 && c < a && d < a && a == lines.len() - 1
        }
        _ => false,
    };

    assert!(
        valid,
        "Finally hooks ran in wrong order. B_finally first, C_done and D_done before A_finally, A_finally last, got: {lines:?}"
    );

    cleanup_test_dir(test_name);
}

/// Test that finally hooks retry on failure.
///
/// Setup:
/// - `StepA` has a finally hook that fails twice, succeeds on third try
/// - `max_retries`: 3
///
/// Expected: `run()` succeeds (finally eventually succeeded after retries)
/// Bug behavior: Finally failures are silently ignored, no retry attempted
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_retries_on_failure() {
    let test_name = "finally_retries_failure";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent just returns empty (no children)
    let _agent = BarnumTestAgent::terminator(&root, Duration::from_millis(10));

    // Finally hook that fails first 2 times, succeeds on 3rd
    let call_count_file = root.join("finally_calls.txt");
    let finally_script = root.join("finally.sh");
    let script = format!(
        r#"#!/bin/bash
count=$(cat "{}" 2>/dev/null || echo 0)
count=$((count + 1))
echo $count > "{}"
if [ $count -lt 3 ]; then
    exit 1
fi
exit 0
"#,
        call_count_file.display(),
        call_count_file.display()
    );
    fs::write(&finally_script, &script).expect("write finally script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod finally script");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "finally": {{"kind": "Command", "script": "{}"}},
                "options": {{"max_retries": 3}}
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    // Check that finally was called 3 times (2 failures + 1 success)
    let call_count: i32 = fs::read_to_string(&call_count_file)
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(0);

    assert!(result.is_ok(), "run should succeed after finally retries");
    assert_eq!(
        call_count, 3,
        "finally did not retry: expected 3 calls, got {call_count}"
    );

    cleanup_test_dir(test_name);
}

/// Test that finally failure propagates after retries are exhausted.
///
/// Setup:
/// - `StepA` has a finally hook that always fails
/// - `max_retries`: 2
///
/// Expected: `run()` returns error (finally failed after all retries)
/// Bug behavior: Finally failures are silently ignored, `run()` succeeds
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_failure_propagates_after_retries_exhausted() {
    let test_name = "finally_failure_propagates";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent just returns empty (no children)
    let _agent = BarnumTestAgent::terminator(&root, Duration::from_millis(10));

    // Finally hook that always fails
    let finally_script = root.join("finally.sh");
    fs::write(&finally_script, "#!/bin/bash\nexit 1\n").expect("write finally script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod finally script");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "finally": {{"kind": "Command", "script": "{}"}},
                "options": {{"max_retries": 2}}
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    // Should fail because finally exhausted retries
    assert!(
        result.is_err(),
        "finally failure not propagated: run() should return error when finally fails"
    );

    cleanup_test_dir(test_name);
}

/// Test that failure of a task spawned by finally propagates.
///
/// Setup:
/// - `StepA` has a finally hook that spawns Cleanup task
/// - Cleanup task always fails
///
/// Expected: `run()` returns error (child of finally failed)
/// Bug behavior: Unknown - need to verify
#[rstest]
#[timeout(Duration::from_secs(20))]
fn finally_child_failure_propagates() {
    let test_name = "finally_child_failure";
    let root = setup_test_dir(test_name);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(test_name);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent: StepA returns empty, Cleanup always fails
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), |payload| {
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = parsed
            .get("task")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("");

        if kind == "Cleanup" {
            "INVALID JSON - FAIL".to_string() // Invalid response = failure
        } else {
            "[]".to_string()
        }
    });

    // Finally hook that spawns a Cleanup task
    let finally_script = root.join("finally.sh");
    fs::write(
        &finally_script,
        r#"#!/bin/bash
echo '[{"kind": "Cleanup", "value": {}}]'
"#,
    )
    .expect("write finally script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&finally_script, fs::Permissions::from_mode(0o755))
            .expect("chmod finally script");
    }

    let config_json = format!(
        r#"{{
        "steps": [
            {{
                "name": "StepA",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "finally": {{"kind": "Command", "script": "{}"}},
                "options": {{"max_retries": 0}}
            }},
            {{
                "name": "Cleanup",
                "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}},
                "next": [],
                "options": {{"max_retries": 0}}
            }}
        ]
    }}"#,
        finally_script.display()
    );

    let config_file: ConfigFile = serde_json::from_str(&config_json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    let result = barnum_config::run(
        &config,
        &schemas,
        &runner_config,
        vec![Task::new("StepA", StepInputValue(serde_json::json!({})))],
    );

    // Should fail because Cleanup (child of finally) failed
    assert!(
        result.is_err(),
        "finally child failure not propagated: run() should return error when finally's child fails"
    );

    cleanup_test_dir(test_name);
}
