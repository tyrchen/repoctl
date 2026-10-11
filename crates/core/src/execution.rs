//! Task execution limits and explicit, opt-in cache contracts.

use serde::{Deserialize, Serialize};

use crate::{CommandSpec, RepoGlob, RepoRelativePath};

/// Explicit inputs and outputs for a reproducible local task.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskCacheConfig {
    /// Enable successful-result caching.
    pub enabled: bool,
    /// Repository-relative input patterns.
    pub inputs: Vec<RepoGlob>,
    /// Repository-relative output roots.
    pub outputs: Vec<RepoRelativePath>,
    /// Names of environment variables affecting the result.
    pub env: Vec<String>,
    /// Bounded runtime/version probes included in the cache key.
    pub runtime: Vec<CommandSpec>,
}

/// Bounded process capture and cache settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskExecutionOptions {
    /// Deadline in seconds, including output capture.
    pub timeout_seconds: u64,
    /// Maximum bytes captured from each output stream.
    pub max_output_bytes: usize,
    /// Explicit caching contract.
    pub cache: TaskCacheConfig,
}

impl Default for TaskExecutionOptions {
    fn default() -> Self {
        Self {
            timeout_seconds: 300,
            max_output_bytes: 1_048_576,
            cache: TaskCacheConfig::default(),
        }
    }
}
