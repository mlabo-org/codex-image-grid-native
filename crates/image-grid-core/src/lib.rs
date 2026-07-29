//! Behavior-bearing contract seed for the native Image Grid runtime.
//!
//! The public values in this crate are intentionally derived from the frozen
//! Electron baseline. The Rust implementation, not the design documents,
//! becomes authoritative as each behavior is implemented.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const APP_IDENTITY: &str = "codex-image-grid-native";
pub const MAX_PROMPTS: usize = 12;
pub const MAX_VARIANTS_PER_PROMPT: u8 = 6;
pub const MAX_RUN_JOBS: usize = 24;
pub const MAX_REFERENCE_IMAGE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_WAIT_MS: u64 = 120_000;
pub const DEFAULT_NATIVE_BIND: &str = "127.0.0.1:4322";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    AppServerImage,
    CodexSvg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    Widescreen,
    Landscape,
    Square,
    Portrait,
    Tall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchShape {
    pub prompt_count: usize,
    pub variants_per_prompt: u8,
    pub total_jobs: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchValidationError {
    PromptsRequired,
    TooManyPrompts,
    PromptEmpty,
    CountOutOfRange,
    TooManyJobs,
}

pub fn validate_batch_shape(
    prompts: &[String],
    variants_per_prompt: u8,
) -> Result<BatchShape, BatchValidationError> {
    if prompts.is_empty() {
        return Err(BatchValidationError::PromptsRequired);
    }
    if prompts.len() > MAX_PROMPTS {
        return Err(BatchValidationError::TooManyPrompts);
    }
    if prompts.iter().any(|prompt| prompt.trim().is_empty()) {
        return Err(BatchValidationError::PromptEmpty);
    }
    if !(1..=MAX_VARIANTS_PER_PROMPT).contains(&variants_per_prompt) {
        return Err(BatchValidationError::CountOutOfRange);
    }

    let total_jobs = prompts.len() * usize::from(variants_per_prompt);
    if total_jobs > MAX_RUN_JOBS {
        return Err(BatchValidationError::TooManyJobs);
    }

    Ok(BatchShape {
        prompt_count: prompts.len(),
        variants_per_prompt,
        total_jobs,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceImageFormat {
    Png,
    Jpeg,
    Webp,
}

impl ReferenceImageFormat {
    fn from_path(path: &Path) -> Result<Self, ReferenceImageError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        match extension.as_str() {
            "png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            _ => Err(ReferenceImageError::UnsupportedExtension),
        }
    }

    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    pub const fn staged_file_name(self) -> &'static str {
        match self {
            Self::Png => "reference.png",
            Self::Jpeg => "reference.jpg",
            Self::Webp => "reference.webp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedReferenceImage {
    pub source_path: PathBuf,
    pub format: ReferenceImageFormat,
    pub media_type: &'static str,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedReferenceImage {
    pub source_path: PathBuf,
    pub staged_path: PathBuf,
    pub format: ReferenceImageFormat,
    pub media_type: &'static str,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceImageError {
    PathNotAbsolute,
    UnsupportedExtension,
    Unavailable { path: PathBuf, reason: String },
    NotRegularFile { path: PathBuf },
    TooLarge { actual: u64, maximum: u64 },
    StageFailed { path: PathBuf, reason: String },
}

impl fmt::Display for ReferenceImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathNotAbsolute => {
                write!(
                    formatter,
                    "referenceImagePath must be an absolute local file path"
                )
            }
            Self::UnsupportedExtension => {
                write!(
                    formatter,
                    "referenceImagePath must point to a PNG, JPEG, or WebP file"
                )
            }
            Self::Unavailable { path, reason } => {
                write!(
                    formatter,
                    "referenceImagePath is unavailable: {} ({reason})",
                    path.display()
                )
            }
            Self::NotRegularFile { path } => {
                write!(
                    formatter,
                    "referenceImagePath must point to a regular file: {}",
                    path.display()
                )
            }
            Self::TooLarge { maximum, .. } => {
                write!(
                    formatter,
                    "reference image is too large; keep it at or below {maximum} bytes"
                )
            }
            Self::StageFailed { path, reason } => {
                write!(
                    formatter,
                    "referenceImagePath could not be staged at {} ({reason})",
                    path.display()
                )
            }
        }
    }
}

impl Error for ReferenceImageError {}

pub fn validate_reference_image(
    path: impl AsRef<Path>,
) -> Result<ValidatedReferenceImage, ReferenceImageError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(ReferenceImageError::PathNotAbsolute);
    }

    let format = ReferenceImageFormat::from_path(path)?;
    let metadata = fs::metadata(path).map_err(|error| ReferenceImageError::Unavailable {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(ReferenceImageError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_REFERENCE_IMAGE_BYTES {
        return Err(ReferenceImageError::TooLarge {
            actual: metadata.len(),
            maximum: MAX_REFERENCE_IMAGE_BYTES,
        });
    }

    let source_path = fs::canonicalize(path).map_err(|error| ReferenceImageError::Unavailable {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    Ok(ValidatedReferenceImage {
        source_path,
        format,
        media_type: format.media_type(),
        size_bytes: metadata.len(),
    })
}

pub fn stage_reference_image(
    source_path: impl AsRef<Path>,
    run_directory: impl AsRef<Path>,
) -> Result<StagedReferenceImage, ReferenceImageError> {
    let validated = validate_reference_image(source_path)?;
    let run_directory = run_directory.as_ref();
    fs::create_dir_all(run_directory).map_err(|error| ReferenceImageError::StageFailed {
        path: run_directory.to_path_buf(),
        reason: error.to_string(),
    })?;

    let staged_path = run_directory.join(validated.format.staged_file_name());
    let mut source =
        File::open(&validated.source_path).map_err(|error| ReferenceImageError::Unavailable {
            path: validated.source_path.clone(),
            reason: error.to_string(),
        })?;
    let mut staged =
        File::create(&staged_path).map_err(|error| ReferenceImageError::StageFailed {
            path: staged_path.clone(),
            reason: error.to_string(),
        })?;

    let copied = io::copy(&mut source.take(MAX_REFERENCE_IMAGE_BYTES + 1), &mut staged).map_err(
        |error| {
            let _ = fs::remove_file(&staged_path);
            ReferenceImageError::StageFailed {
                path: staged_path.clone(),
                reason: error.to_string(),
            }
        },
    )?;
    if copied > MAX_REFERENCE_IMAGE_BYTES {
        drop(staged);
        let _ = fs::remove_file(&staged_path);
        return Err(ReferenceImageError::TooLarge {
            actual: copied,
            maximum: MAX_REFERENCE_IMAGE_BYTES,
        });
    }
    staged
        .flush()
        .map_err(|error| ReferenceImageError::StageFailed {
            path: staged_path.clone(),
            reason: error.to_string(),
        })?;
    drop(staged);

    let staged_path =
        fs::canonicalize(&staged_path).map_err(|error| ReferenceImageError::StageFailed {
            path: staged_path.clone(),
            reason: error.to_string(),
        })?;

    Ok(StagedReferenceImage {
        source_path: validated.source_path,
        staged_path,
        format: validated.format,
        media_type: validated.media_type,
        size_bytes: copied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn prompts(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("prompt {index}")).collect()
    }

    #[test]
    fn accepts_the_frozen_baseline_maximum() {
        let result = validate_batch_shape(&prompts(4), 6).expect("maximum accepted batch");
        assert_eq!(result.total_jobs, 24);
    }

    #[test]
    fn rejects_batches_above_the_global_job_cap() {
        assert_eq!(
            validate_batch_shape(&prompts(5), 5),
            Err(BatchValidationError::TooManyJobs)
        );
    }

    #[test]
    fn rejects_blank_prompts() {
        assert_eq!(
            validate_batch_shape(&["  ".to_owned()], 1),
            Err(BatchValidationError::PromptEmpty)
        );
    }

    #[test]
    fn stages_a_jpeg_with_the_frozen_destination_name() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let external_directory = temporary.path().join("external");
        let run_directory = temporary.path().join("generated").join("run-1234");
        fs::create_dir_all(&external_directory).expect("external directory");
        let source_path = external_directory.join("user-selected-name.jpeg");
        fs::write(&source_path, b"original reference bytes").expect("reference fixture");

        let staged = stage_reference_image(&source_path, &run_directory).expect("reference staged");

        assert_eq!(
            staged
                .staged_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some("reference.jpg")
        );
        assert_eq!(staged.media_type, "image/jpeg");
        assert_eq!(
            fs::read(&staged.staged_path).expect("staged bytes"),
            b"original reference bytes"
        );

        fs::write(&source_path, b"mutated external bytes").expect("mutated source");
        assert_eq!(
            fs::read(&staged.staged_path).expect("stable staged bytes"),
            b"original reference bytes"
        );
    }

    #[test]
    fn replaces_an_existing_staged_reference() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source_path = temporary.path().join("source.png");
        let run_directory = temporary.path().join("generated").join("run-1234");
        fs::create_dir_all(&run_directory).expect("run directory");
        fs::write(&source_path, b"new bytes").expect("reference fixture");
        fs::write(run_directory.join("reference.png"), b"old bytes").expect("old staged reference");

        let staged = stage_reference_image(&source_path, &run_directory).expect("reference staged");

        assert_eq!(
            fs::read(staged.staged_path).expect("replaced bytes"),
            b"new bytes"
        );
    }

    #[test]
    fn rejects_invalid_reference_paths_before_staging() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory_path = temporary.path().join("folder.png");
        fs::create_dir_all(&directory_path).expect("fixture directory");
        let unsupported_path = temporary.path().join("reference.gif");
        fs::write(&unsupported_path, b"gif").expect("unsupported fixture");

        assert_eq!(
            validate_reference_image("relative.png"),
            Err(ReferenceImageError::PathNotAbsolute)
        );
        assert!(matches!(
            validate_reference_image(temporary.path().join("missing.png")),
            Err(ReferenceImageError::Unavailable { .. })
        ));
        let directory_error =
            validate_reference_image(&directory_path).expect_err("directory must be rejected");
        assert_eq!(
            directory_error,
            ReferenceImageError::NotRegularFile {
                path: directory_path.clone()
            }
        );
        assert_eq!(
            directory_error.to_string(),
            format!(
                "referenceImagePath must point to a regular file: {}",
                directory_path.display()
            )
        );
        assert_eq!(
            validate_reference_image(&unsupported_path),
            Err(ReferenceImageError::UnsupportedExtension)
        );
    }

    #[test]
    fn accepts_exactly_one_hundred_mib_and_rejects_one_byte_more() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let accepted_path = temporary.path().join("accepted.webp");
        let rejected_path = temporary.path().join("rejected.webp");
        let mut accepted = File::create(&accepted_path).expect("accepted fixture");
        accepted
            .set_len(MAX_REFERENCE_IMAGE_BYTES)
            .expect("accepted size");
        accepted.flush().expect("accepted fixture flushed");
        let mut rejected = File::create(&rejected_path).expect("rejected fixture");
        rejected
            .set_len(MAX_REFERENCE_IMAGE_BYTES + 1)
            .expect("rejected size");
        rejected.flush().expect("rejected fixture flushed");

        assert_eq!(
            validate_reference_image(&accepted_path)
                .expect("maximum accepted")
                .size_bytes,
            MAX_REFERENCE_IMAGE_BYTES
        );
        let size_error =
            validate_reference_image(&rejected_path).expect_err("oversized image must be rejected");
        assert_eq!(
            size_error,
            ReferenceImageError::TooLarge {
                actual: MAX_REFERENCE_IMAGE_BYTES + 1,
                maximum: MAX_REFERENCE_IMAGE_BYTES,
            }
        );
        assert_eq!(
            size_error.to_string(),
            format!(
                "reference image is too large; keep it at or below {MAX_REFERENCE_IMAGE_BYTES} bytes"
            )
        );
    }
}
