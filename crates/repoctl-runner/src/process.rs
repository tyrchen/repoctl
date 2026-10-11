//! Deadline-bounded argv execution with bounded pipe capture and child cleanup.

use std::{io, path::Path, process::Stdio, time::Duration};

use repoctl_core::{ProcessCommand, ProcessOutput, ProcessRunner, RepoctlError};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    runtime::Builder,
    time::timeout,
};

/// Local process runner with bounded execution and output.
#[derive(Clone, Debug, Default)]
pub struct LocalProcessRunner;

impl ProcessRunner for LocalProcessRunner {
    fn run(&self, command: &ProcessCommand) -> Result<ProcessOutput, RepoctlError> {
        let output = run_bytes(command)?;
        Ok(ProcessOutput {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            ..ProcessOutput::default()
        })
    }
}

#[derive(Debug)]
pub(super) struct CapturedOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(super) fn run_bytes(command: &ProcessCommand) -> Result<CapturedOutput, RepoctlError> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RepoctlError::Environment(format!("process runtime: {error}")))?;
    runtime.block_on(capture_process(command))
}

async fn capture_process(command: &ProcessCommand) -> Result<CapturedOutput, RepoctlError> {
    let options = &command.execution;
    if !(1..=86_400).contains(&options.timeout_seconds)
        || !(1..=16_777_216).contains(&options.max_output_bytes)
    {
        return Err(RepoctlError::Environment(
            "invalid process execution limits".to_string(),
        ));
    }
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .envs(&command.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let cwd = command
        .absolute_cwd
        .as_deref()
        .unwrap_or_else(|| Path::new(command.cwd.as_str()));
    if let Some(root) = &command.repo_root {
        let canonical = tokio::fs::canonicalize(cwd)
            .await
            .map_err(|error| RepoctlError::io(cwd.display().to_string(), error))?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|error| RepoctlError::io(root.display().to_string(), error))?;
        if !canonical.starts_with(root) {
            return Err(RepoctlError::Environment(
                "process working directory escaped repository".to_string(),
            ));
        }
    }
    process.current_dir(cwd);
    let mut child = process.spawn().map_err(|error| {
        RepoctlError::Environment(format!("failed to start `{}`: {error}", command.program))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RepoctlError::Internal("child stdout missing".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RepoctlError::Internal("child stderr missing".to_string()))?;
    let result = Box::pin(timeout(
        Duration::from_secs(options.timeout_seconds),
        async {
            tokio::try_join!(
                child.wait(),
                capture(stdout, options.max_output_bytes),
                capture(stderr, options.max_output_bytes)
            )
        },
    ))
    .await;
    match result {
        Ok(Ok((status, stdout, stderr))) => Ok(CapturedOutput {
            status: status.code().unwrap_or(1),
            stdout,
            stderr,
        }),
        failure => {
            // Always request termination and reap before returning from the adapter.
            let _ = child.kill().await;
            let _ = child.wait().await;
            let (status, message) = match failure {
                Err(_) => (
                    124,
                    format!(
                        "process exceeded {} second deadline",
                        options.timeout_seconds
                    ),
                ),
                Ok(Err(error)) => (125, format!("process output capture failed: {error}")),
                Ok(Ok(_)) => {
                    return Err(RepoctlError::Internal(
                        "unexpected process completion".to_string(),
                    ));
                }
            };
            Ok(CapturedOutput {
                status,
                stdout: Vec::new(),
                stderr: message.into_bytes(),
            })
        }
    }
}

async fn capture(mut stream: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(io::Error::other("output limit exceeded"));
        }
        if let Some(chunk) = buffer.get(..read) {
            bytes.extend_from_slice(chunk);
        }
    }
}
