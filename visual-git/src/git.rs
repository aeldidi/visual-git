//! Abstraction for the git backend we use. This is just in case I decide to
//! switch between using the git binary and libgit2.

use std::{path::Path, process::Command};

use nanoserde::SerJson;

use crate::util;

#[derive(SerJson, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub code: String,
}

#[derive(SerJson, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub repo_path: String,
    pub branch: String,
    pub clean: bool,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub updated_unix_ms: u64,
    pub error: String,
    pub entries: Vec<StatusEntry>,
}

pub trait Backend: Send + Sync {
    fn read_status(&self, repo_path: &Path) -> StatusSnapshot;
}

pub struct CliBackend;

impl CliBackend {
    pub fn new() -> Self {
        Self
    }

    fn read_status_inner(
        &self,
        repo_path: &Path,
    ) -> Result<StatusSnapshot, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ])
            .output()
            .map_err(|err| format!("failed to run git status: {}", err))?;

        if !output.status.success() {
            let stderr =
                String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("git status exited with {}", output.status)
            } else {
                stderr
            };
            return Err(format!("failed to run git status: {}", detail));
        }

        parse_porcelain_v2(repo_path, &output.stdout)
    }
}

impl Backend for CliBackend {
    fn read_status(&self, repo_path: &Path) -> StatusSnapshot {
        match self.read_status_inner(repo_path) {
            Ok(snapshot) => snapshot,
            Err(error) => StatusSnapshot {
                repo_path: repo_path.display().to_string(),
                branch: "DETACHED".to_string(),
                clean: false,
                staged: 0,
                unstaged: 0,
                untracked: 0,
                updated_unix_ms: util::now_unix_ms(),
                error,
                entries: Vec::new(),
            },
        }
    }
}

fn parse_porcelain_v2(
    repo_path: &Path,
    stdout: &[u8],
) -> Result<StatusSnapshot, String> {
    let mut snapshot = StatusSnapshot {
        repo_path: repo_path.display().to_string(),
        branch: "DETACHED".to_string(),
        clean: true,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        updated_unix_ms: util::now_unix_ms(),
        error: String::new(),
        entries: Vec::new(),
    };

    let mut records = stdout.split(|b| *b == 0).peekable();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }

        if let Some(head) = record.strip_prefix(b"# branch.head ") {
            let branch = String::from_utf8_lossy(head).into_owned();
            snapshot.branch = match branch.as_str() {
                "(detached)" => "DETACHED".to_string(),
                "(unknown)" => "UNKNOWN".to_string(),
                _ => branch,
            };
            continue;
        }

        let tag = record[0];
        match tag {
            b'1' => {
                let (code, path) =
                    parse_code_and_path(record, 9).ok_or_else(|| {
                        format!("invalid status record: {}", to_lossy(record))
                    })?;
                add_status_entry(&mut snapshot, code, to_lossy(path));
            }
            b'2' => {
                let (code, path) =
                    parse_code_and_path(record, 10).ok_or_else(|| {
                        format!(
                            "invalid rename/copy record: {}",
                            to_lossy(record)
                        )
                    })?;
                let original = records.next().ok_or_else(|| {
                    format!("missing original path for: {}", to_lossy(record))
                })?;
                add_status_entry(
                    &mut snapshot,
                    code,
                    format!("{} -> {}", to_lossy(original), to_lossy(path)),
                );
            }
            b'u' => {
                let (code, path) =
                    parse_code_and_path(record, 10).ok_or_else(|| {
                        format!("invalid unmerged record: {}", to_lossy(record))
                    })?;
                add_status_entry(&mut snapshot, code, to_lossy(path));
            }
            b'?' => {
                let path = record.strip_prefix(b"? ").ok_or_else(|| {
                    format!("invalid untracked record: {}", to_lossy(record))
                })?;
                add_status_entry(
                    &mut snapshot,
                    "??".to_string(),
                    to_lossy(path),
                );
            }
            b'!' => {
                let path = record.strip_prefix(b"! ").ok_or_else(|| {
                    format!("invalid ignored record: {}", to_lossy(record))
                })?;
                add_status_entry(
                    &mut snapshot,
                    "!!".to_string(),
                    to_lossy(path),
                );
            }
            _ => {}
        }
    }

    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.clean = snapshot.entries.is_empty();
    Ok(snapshot)
}

fn add_status_entry(snapshot: &mut StatusSnapshot, code: String, path: String) {
    if code == "??" {
        snapshot.untracked += 1;
    } else if is_conflict_code(&code) {
        snapshot.staged += 1;
        snapshot.unstaged += 1;
    } else if let [index, worktree] = code.as_bytes() {
        if *index != b' ' {
            snapshot.staged += 1;
        }
        if *worktree != b' ' {
            snapshot.unstaged += 1;
        }
    }

    snapshot.entries.push(StatusEntry { path, code });
}

fn is_conflict_code(code: &str) -> bool {
    matches!(code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
}

fn parse_code_and_path(
    record: &[u8],
    split_count: usize,
) -> Option<(String, &[u8])> {
    let mut parts = record.splitn(split_count, |b| *b == b' ');
    parts.next()?;
    let code = to_lossy(parts.next()?);

    for _ in 0..(split_count.saturating_sub(3)) {
        parts.next()?;
    }

    let path = parts.next()?;
    Some((code, path))
}

fn to_lossy(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}
