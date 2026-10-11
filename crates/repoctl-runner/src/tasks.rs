//! Ready-node scheduling with scoped workers and deterministic reporting.

use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, mpsc},
    thread,
};

use repoctl_core::{ProcessCommand, ProcessOutput, ProcessRunner, RepoSnapshot, RepoctlError};

use super::cache;

pub(super) fn run_process_commands(
    runner: &Arc<dyn ProcessRunner>,
    commands: &[ProcessCommand],
    concurrency: usize,
    snapshot: &Arc<RepoSnapshot>,
    no_cache: bool,
) -> Result<Vec<(ProcessCommand, ProcessOutput)>, RepoctlError> {
    if !(1..=256).contains(&concurrency) {
        return Err(RepoctlError::Environment(
            "task concurrency must be 1..=256".to_string(),
        ));
    }
    let groups = group_commands(commands)?;
    let mut pending = groups.keys().cloned().collect::<BTreeSet<_>>();
    let mut completed = BTreeMap::<String, bool>::new();
    let mut results = BTreeMap::new();
    let mut fingerprints = BTreeMap::<String, String>::new();
    thread::scope(|scope| -> Result<(), RepoctlError> {
        let (sender, receiver) = mpsc::channel();
        let mut running = 0;
        while !pending.is_empty() || running > 0 {
            let ready = ready_tasks(&pending, &groups, &completed);
            let made_progress = !ready.is_empty();
            for id in ready {
                if running >= concurrency {
                    break;
                }
                let mut group = groups
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| RepoctlError::Internal("task group missing".to_string()))?;
                pending.remove(&id);
                let blocked = group
                    .iter()
                    .flat_map(|(_, c)| &c.prerequisites)
                    .any(|dependency| completed.get(dependency) == Some(&false));
                if blocked {
                    skip_group(group, &mut results);
                    completed.insert(id, false);
                    continue;
                }
                attach_fingerprints(&mut group, &fingerprints);
                let sender = sender.clone();
                let runner = Arc::clone(runner);
                let snapshot = Arc::clone(snapshot);
                running += 1;
                scope.spawn(move || {
                    let (success, outputs) = execute_group(group, &runner, &snapshot, no_cache);
                    let _ = sender.send((id, success, outputs));
                });
            }
            if running > 0 {
                let (id, success, outputs) = receiver.recv().map_err(|error| {
                    RepoctlError::Internal(format!("task completion channel: {error}"))
                })?;
                running -= 1;
                store_completion(
                    id,
                    success,
                    outputs,
                    &mut fingerprints,
                    &mut completed,
                    &mut results,
                );
            } else if !pending.is_empty() && !made_progress {
                return Err(RepoctlError::Environment(
                    "task graph contains a cycle or missing prerequisite".to_string(),
                ));
            }
        }
        Ok(())
    })?;
    Ok(results.into_values().collect())
}

fn skip_group(
    group: Vec<(usize, ProcessCommand)>,
    results: &mut BTreeMap<usize, (ProcessCommand, ProcessOutput)>,
) {
    for (index, command) in group {
        results.insert(
            index,
            (command, failed(125, "skipped: prerequisite task failed")),
        );
    }
}

fn ready_tasks(
    pending: &BTreeSet<String>,
    groups: &BTreeMap<String, Vec<(usize, ProcessCommand)>>,
    completed: &BTreeMap<String, bool>,
) -> Vec<String> {
    pending
        .iter()
        .filter(|id| {
            groups.get(*id).is_some_and(|group| {
                group
                    .iter()
                    .flat_map(|(_, c)| &c.prerequisites)
                    .all(|dependency| completed.contains_key(dependency))
            })
        })
        .cloned()
        .collect::<Vec<_>>()
}

fn attach_fingerprints(
    group: &mut [(usize, ProcessCommand)],
    fingerprints: &BTreeMap<String, String>,
) {
    for (_, command) in group {
        command.prerequisite_fingerprints = command
            .prerequisites
            .iter()
            .filter_map(|dependency| fingerprints.get(dependency).cloned())
            .collect();
    }
}

fn store_completion(
    id: String,
    success: bool,
    outputs: GroupOutputs,
    fingerprints: &mut BTreeMap<String, String>,
    completed: &mut BTreeMap<String, bool>,
    results: &mut BTreeMap<usize, (ProcessCommand, ProcessOutput)>,
) {
    let mut digest = blake3::Hasher::new();
    let mut artifact_aware = success;
    for (_, _, output) in &outputs {
        if let Some(fingerprint) = &output.fingerprint {
            digest.update(fingerprint.as_bytes());
        } else {
            artifact_aware = false;
        }
    }
    if artifact_aware {
        fingerprints.insert(id.clone(), digest.finalize().to_hex().to_string());
    }
    completed.insert(id, success);
    for (index, command, output) in outputs {
        results.insert(index, (command, output));
    }
}

fn group_commands(
    commands: &[ProcessCommand],
) -> Result<BTreeMap<String, Vec<(usize, ProcessCommand)>>, RepoctlError> {
    let mut groups = BTreeMap::<String, Vec<(usize, ProcessCommand)>>::new();
    for (index, command) in commands.iter().enumerate() {
        groups
            .entry(identity(command)?)
            .or_default()
            .push((index, command.clone()));
    }
    Ok(groups)
}

type GroupOutputs = Vec<(usize, ProcessCommand, ProcessOutput)>;

fn execute_group(
    group: Vec<(usize, ProcessCommand)>,
    runner: &Arc<dyn ProcessRunner>,
    snapshot: &RepoSnapshot,
    no_cache: bool,
) -> (bool, GroupOutputs) {
    let mut outputs = Vec::new();
    let mut success = true;
    let mut preceding_fingerprints = Vec::with_capacity(group.len());
    for (index, command) in group {
        let output = if success {
            match catch_unwind(AssertUnwindSafe(|| {
                cache::run(
                    runner,
                    &command,
                    snapshot,
                    no_cache,
                    &preceding_fingerprints,
                )
            })) {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => failed(126, &error.to_string()),
                Err(_) => failed(127, "task worker panicked"),
            }
        } else {
            failed(125, "skipped: an earlier command in this task failed")
        };
        success &= output.status == 0;
        preceding_fingerprints.push(output.fingerprint.clone());
        outputs.push((index, command, output));
    }
    (success, outputs)
}

fn identity(command: &ProcessCommand) -> Result<String, RepoctlError> {
    match (&command.project, &command.workspace, &command.task) {
        (Some(project), Some(workspace), Some(task)) => Ok(format!("{project}:{workspace}:{task}")),
        _ => Err(RepoctlError::Internal(
            "scheduled command is missing task identity".to_string(),
        )),
    }
}

fn failed(status: i32, message: &str) -> ProcessOutput {
    ProcessOutput {
        status,
        stderr: message.to_string(),
        ..ProcessOutput::default()
    }
}
