//! Bounded source-byte commitments for reviewed migration operations.

// Adoption is synchronous CLI orchestration, outside an async runtime.
#![allow(clippy::disallowed_types)]

use std::{
    fs::File,
    io::{Read as _, copy},
};

use blake3::Hasher;
use camino::Utf8Path;
use repoctl_core::{Diagnostic, RepoctlError};

const MAX_FILE_BYTES: u64 = 268_435_456;
const MAX_TOTAL_BYTES: u64 = 1_073_741_824;

#[derive(Debug, Default)]
pub(super) struct DigestBudget {
    bytes: u64,
}

impl DigestBudget {
    pub fn digest(&mut self, path: &Utf8Path) -> Result<String, RepoctlError> {
        let file = File::open(path).map_err(|error| RepoctlError::io(path.to_path_buf(), error))?;
        let metadata = file
            .metadata()
            .map_err(|error| RepoctlError::io(path.to_path_buf(), error))?;
        if !metadata.is_file() {
            return Err(RepoctlError::diagnostic(
                Diagnostic::error("adoption.copy.source", "copy source must be a regular file")
                    .with_path(path.as_str()),
            ));
        }
        let available = MAX_FILE_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(self.bytes));
        if metadata.len() > available {
            return Err(copy_limit(path));
        }
        let mut reader = file.take(available.saturating_add(1));
        let mut hasher = Hasher::new();
        let size = copy(&mut reader, &mut hasher)
            .map_err(|error| RepoctlError::io(path.to_path_buf(), error))?;
        self.bytes = self.bytes.saturating_add(size);
        if size > available {
            return Err(copy_limit(path));
        }
        Ok(format!("blake3:{}", hasher.finalize().to_hex()))
    }
}

fn copy_limit(path: &Utf8Path) -> RepoctlError {
    RepoctlError::diagnostic(
        Diagnostic::error(
            "adoption.limit.copy_bytes",
            "source copies exceed 256 MiB per file or 1 GiB total",
        )
        .with_path(path.as_str()),
    )
}

pub(super) fn validate_checksum<'a>(
    checksum: Option<&'a str>,
    source: &Utf8Path,
) -> Result<&'a str, RepoctlError> {
    checksum
        .filter(|value| {
            value.strip_prefix("blake3:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        })
        .ok_or_else(|| {
            RepoctlError::diagnostic(
                Diagnostic::error(
                    "adoption.copy.checksum",
                    "source operation requires a valid BLAKE3 checksum; regenerate the adoption \
                     plan",
                )
                .with_path(source.as_str()),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs::{self, File},
    };

    use camino::Utf8Path;

    use super::{DigestBudget, MAX_FILE_BYTES, MAX_TOTAL_BYTES, validate_checksum};

    #[test]
    fn test_should_bound_aggregate_source_bytes_and_validate_digest_format()
    -> Result<(), Box<dyn Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("binary");
        fs::write(&path, [0, 255])?;
        let path = Utf8Path::from_path(&path).ok_or("non-UTF8 test path")?;
        let mut budget = DigestBudget {
            bytes: MAX_TOTAL_BYTES - 1,
        };
        assert!(budget.digest(path).is_err());
        let digest = DigestBudget::default().digest(path)?;
        assert_eq!(validate_checksum(Some(&digest), path)?, digest);
        for invalid in [None, Some(""), Some("sha256:abc"), Some("blake3:abc")] {
            assert!(validate_checksum(invalid, path).is_err());
        }
        File::create(path)?.set_len(MAX_FILE_BYTES + 1)?;
        assert!(DigestBudget::default().digest(path).is_err());
        Ok(())
    }
}
