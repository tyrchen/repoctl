//! Required dependency maintenance after reviewed repository migration.

use serde::{Deserialize, Serialize};

use crate::{ProcessCommand, RepoRelativePath};

/// A lockfile invalidated by dependency rewrites and its explicit regeneration command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "camelCase")]
pub struct LockfileRefresh {
    /// Destination lockfile that must be regenerated before verification.
    pub lockfile: RepoRelativePath,
    /// Reviewed command to run after applying the file operations.
    pub command: ProcessCommand,
}

impl LockfileRefresh {
    /// Records a required refresh without executing the package manager.
    ///
    /// ```
    /// use repoctl_core::{LockfileRefresh, ProcessCommand, RepoRelativePath};
    /// let refresh = LockfileRefresh::new(RepoRelativePath::root(), ProcessCommand::default());
    /// assert_eq!(refresh.lockfile.as_str(), ".");
    /// ```
    pub fn new(lockfile: RepoRelativePath, command: ProcessCommand) -> Self {
        Self { lockfile, command }
    }
}
