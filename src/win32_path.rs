use std::path::{Path, PathBuf};

/// Ensure a path is usable with `std::fs` even when it exceeds MAX_PATH
/// (260 chars) by prefixing with `\\?\` when necessary.
///
/// Relative paths are resolved against the current directory first.
/// Non-Windows platforms return the path unchanged.
#[cfg(target_os = "windows")]
pub fn to_fs_path<P: AsRef<Path>>(path: P) -> PathBuf {
    const MAX_SAFE: usize = 240;
    let path = path.as_ref();

    let s = path.to_string_lossy();
    if s.starts_with(r"\\?\") {
        return path.to_path_buf();
    }

    // Fully qualify and lexically normalize BEFORE any verbatim prefix is
    // added: `\\?\` paths bypass Windows normalization, so forward slashes
    // (ubiquitous in Git paths) and `.`/`..` segments must already be
    // resolved to native form, otherwise filesystem operations fail.
    let abs = std::path::absolute(path).unwrap_or_else(|_| {
        if path.is_relative() {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(path),
                Err(_) => path.to_path_buf(),
            }
        } else {
            path.to_path_buf()
        }
    });

    let abs_s = abs.to_string_lossy();
    if abs_s.len() > MAX_SAFE {
        if let Some(unc) = abs_s.strip_prefix(r"\\") {
            PathBuf::from(format!(r"\\?\UNC\{}", unc.trim_start_matches(r"\\")))
        } else {
            PathBuf::from(format!(r"\\?\{}", abs_s))
        }
    } else {
        abs
    }
}

#[cfg(not(target_os = "windows"))]
pub fn to_fs_path<P: AsRef<Path>>(path: P) -> PathBuf {
    path.as_ref().to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_passes_through() {
        let p = to_fs_path("C:\\short\\path.txt");
        // On non-Windows the prefix comparison may not apply, but
        // the path should be unchanged in any case.
        assert_eq!(p.to_string_lossy(), "C:\\short\\path.txt");
    }

    #[test]
    fn already_prefixed_path_is_not_double_prefixed() {
        let p = to_fs_path(r"\\?\C:\very\long\path.txt");
        assert_eq!(p.to_string_lossy(), r"\\?\C:\very\long\path.txt");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn long_absolute_path_gets_prefix() {
        let long = "C:\\".to_string() + &"a".repeat(250);
        let p = to_fs_path(&long);
        let s = p.to_string_lossy();
        assert!(
            s.starts_with(r"\\?\C:\"),
            "expected \\\\?\\ prefix, got {s}"
        );
        assert_eq!(s.len(), long.len() + 4, "prefix adds 4 chars");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn unc_path_gets_unc_prefix() {
        let unc = r"\\server\share\".to_string() + &"a".repeat(250);
        let p = to_fs_path(&unc);
        let s = p.to_string_lossy();
        assert!(
            s.starts_with(r"\\?\UNC\server\share"),
            "expected \\\\?\\UNC\\ prefix, got {s}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn relative_path_is_resolved_and_prefixed_when_long() {
        let long_rel = "a".repeat(250);
        let p = to_fs_path(&long_rel);
        let s = p.to_string_lossy();
        assert!(s.starts_with(r"\\?\"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn long_forward_slash_git_path_is_normalized_before_prefix() {
        // Git hands us repo-relative paths with '/' separators. Once the
        // verbatim prefix is added Windows stops normalizing them, so the
        // result must already be fully qualified with native separators.
        let rel = format!("src/{}file.txt", "deep\\sub/".repeat(40));
        let p = to_fs_path(&rel);
        let s = p.to_string_lossy();
        assert!(s.starts_with(r"\\?\"), "expected verbatim prefix, got {s}");
        assert!(
            !s.contains('/'),
            "forward slashes must be normalized before prefixing: {s}"
        );
        assert!(
            s.starts_with(r"\\?\") && p.is_absolute(),
            "prefixed path must be fully qualified"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_returns_path_unchanged() {
        let p = to_fs_path("/some/long/path");
        assert_eq!(p.to_string_lossy(), "/some/long/path");
    }

    #[test]
    fn relative_short_path_passes_through() {
        let p = to_fs_path("src/main.rs");
        let s = p.to_string_lossy();
        // Should be unchanged if short, resolved to absolute if needed.
        assert!(!s.starts_with(r"\\?\"));
    }
}
