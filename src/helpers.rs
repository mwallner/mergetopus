use anyhow::{Context, Result, bail};

fn extract_trailer(message: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    message
        .lines()
        .find(|line| line.starts_with(&prefix))
        .map(|line| line[prefix.len()..].trim().to_string())
}

pub fn extract_slice_paths(message: &str) -> Vec<String> {
    if let Some(val) = extract_trailer(message, "Slice-Paths") {
        let paths: Vec<String> = val
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if !paths.is_empty() {
            return paths;
        }
    }

    message
        .lines()
        .filter(|line| line.starts_with("Source-Path: "))
        .map(|line| line["Source-Path: ".len()..].trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a command string into program and arguments, handling quoted tokens.
/// Splits on unquoted whitespace to handle paths and arguments with special characters.
fn parse_command_string(cmd: &str) -> Result<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut in_double_quotes = false;
    let mut in_single_quotes = false;

    for ch in cmd.chars() {
        match ch {
            '"' if !in_single_quotes => {
                in_double_quotes = !in_double_quotes;
            }
            '\'' if !in_double_quotes => {
                in_single_quotes = !in_single_quotes;
            }
            ' ' | '\t' if !in_double_quotes && !in_single_quotes => {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
            }
            _ => current_token.push(ch),
        }
    }

    if !current_token.is_empty() {
        tokens.push(current_token);
    }

    if tokens.is_empty() {
        bail!("empty merge tool command");
    }

    let program = tokens.remove(0);
    Ok((program, tokens))
}

fn is_windows_shell_builtin(program: &str) -> bool {
    matches!(
        program.to_ascii_lowercase().as_str(),
        "assoc"
            | "break"
            | "call"
            | "cd"
            | "chcp"
            | "cls"
            | "color"
            | "copy"
            | "date"
            | "del"
            | "dir"
            | "echo"
            | "endlocal"
            | "erase"
            | "exit"
            | "for"
            | "ftype"
            | "if"
            | "md"
            | "mkdir"
            | "mklink"
            | "move"
            | "path"
            | "pause"
            | "popd"
            | "prompt"
            | "pushd"
            | "rd"
            | "ren"
            | "rename"
            | "rmdir"
            | "set"
            | "setlocal"
            | "shift"
            | "start"
            | "time"
            | "title"
            | "type"
            | "ver"
            | "verify"
            | "vol"
    )
}

fn needs_windows_shell(expanded_cmd: &str, program: &str) -> bool {
    is_windows_shell_builtin(program)
        || expanded_cmd
            .chars()
            .any(|c| matches!(c, '>' | '<' | '|' | '&' | '^'))
}

pub fn run_windows_merge_tool(
    tool_name: &str,
    expanded_cmd: &str,
) -> Result<std::process::ExitStatus> {
    let (program, args) = parse_command_string(expanded_cmd)?;

    match std::process::Command::new(&program).args(&args).status() {
        Ok(status) => Ok(status),
        Err(err)
            if err.kind() == std::io::ErrorKind::NotFound
                && needs_windows_shell(expanded_cmd, &program) =>
        {
            cmd_shell_execute(tool_name, expanded_cmd)
        }
        Err(err) => Err(err).with_context(|| {
            format!("failed to launch merge tool '{tool_name}' (program: '{program}')")
        }),
    }
}

/// Run a command through `cmd /d /s /c` on Windows.
///
/// Uses `raw_arg` to pass the command string without Rust's MSVC-style quote
/// escaping, which would otherwise turn `"path"` into `\"path\"` and garble
/// paths for cmd.exe (cmd treats `\` as a literal, not an escape character).
#[cfg(target_os = "windows")]
fn cmd_shell_execute(tool_name: &str, expanded_cmd: &str) -> Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/d", "/s", "/c"])
        .raw_arg(expanded_cmd)
        .status()
        .with_context(|| {
            format!("failed to launch merge tool '{tool_name}' via Windows shell fallback")
        })
}

#[cfg(not(target_os = "windows"))]
fn cmd_shell_execute(tool_name: &str, expanded_cmd: &str) -> Result<std::process::ExitStatus> {
    std::process::Command::new("cmd")
        .args(["/d", "/s", "/c", expanded_cmd])
        .status()
        .with_context(|| {
            format!("failed to launch merge tool '{tool_name}' via Windows shell fallback")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trailer_finds_key() {
        let msg = "Mergetopus slice: 1 file(s) from 'main' (theirs)\n\nSource-Ref: origin/feature\nSource-Commit: abc123\nSource-Path: src/foo.rs\nSource-Path-Commit: def456\n";
        assert_eq!(
            extract_trailer(msg, "Source-Ref"),
            Some("origin/feature".to_string())
        );
        assert_eq!(
            extract_trailer(msg, "Source-Commit"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_trailer(msg, "Missing-Key"), None);
    }

    #[test]
    fn extract_slice_paths_single_file() {
        let msg = "Mergetopus slice: 'src/foo.rs' from 'main' (theirs)\n\nSource-Ref: main\nSource-Commit: abc123\nSource-Path: src/foo.rs\nSource-Path-Commit: def456\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/foo.rs".to_string()]);
    }

    #[test]
    fn extract_slice_paths_explicit_multi_file() {
        let msg = "Mergetopus slice: 2 file(s) from 'main' (theirs)\n\nSource-Ref: main\nSource-Commit: abc123\nSlice-Paths: src/a.rs, src/b.rs\nSource-Path: src/a.rs\nSource-Path-Commit: x\nSource-Path: src/b.rs\nSource-Path-Commit: y\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
    }

    #[test]
    fn extract_slice_paths_does_not_match_source_path_commit() {
        let msg = "Mergetopus slice: 'src/lib.rs' from 'feat' (theirs)\n\nSource-Ref: feat\nSource-Commit: 111\nSource-Path: src/lib.rs\nSource-Path-Commit: 222\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/lib.rs".to_string()]);
    }
}
