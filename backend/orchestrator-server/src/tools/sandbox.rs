//! The workspace sandbox: every file path is relative to one root and stays inside it.
//! `relative` is lexical and does no I/O, for `check`; `resolve` also rejects symlinks.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use orchestrator_core::tools::ToolError;

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidArgs(format!("path: {message}"))
}

/// Rejects empty, absolute and `..` paths. `.` components are dropped.
pub fn relative(path: &str) -> Result<PathBuf, ToolError> {
    if path.trim().is_empty() {
        return Err(invalid("must not be empty"));
    }
    if path.starts_with(['/', '\\']) || Path::new(path).is_absolute() {
        return Err(invalid("must be relative to the workspace"));
    }
    let mut clean = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(invalid(".. is not allowed")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid("must be relative to the workspace"));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Ok(PathBuf::from("."));
    }
    Ok(clean)
}

/// The path under `root`, after checking that no existing component of it is a symlink.
/// `symlink_metadata` reports the link itself, where `metadata` would follow it out.
pub async fn resolve(root: &Path, path: &str) -> Result<PathBuf, ToolError> {
    let relative = relative(path)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(invalid("symlinks are not allowed"));
            }
            Ok(_) => {}
            // A missing component cannot be a symlink, and neither can anything below it.
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(_) => return Err(ToolError::Failed("the path cannot be read".to_string())),
        }
    }
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_rejects_escapes_and_keeps_normal_paths() {
        for bad in [
            "",
            "/etc/passwd",
            "\\\\server\\share",
            "../x",
            "a/../../x",
            "a/..",
        ] {
            assert!(relative(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            relative("./data/a.md").ok(),
            Some(PathBuf::from("data/a.md"))
        );
    }
}
