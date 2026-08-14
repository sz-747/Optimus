use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChange {
    pub file_key: String,
    pub lines_added: Option<u64>,
    pub lines_deleted: Option<u64>,
    pub binary: bool,
    pub untracked: bool,
}

#[derive(Debug)]
pub enum GitError {
    Io(std::io::Error),
    Command(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "git: {error}"),
            Self::Command(error) => write!(f, "git: {error}"),
        }
    }
}

impl std::error::Error for GitError {}

impl From<std::io::Error> for GitError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub trait GitWorktrees: Send + Sync {
    fn resolve_main(&self, repo_root: &Path) -> Result<String, GitError>;
    fn create_worktree(
        &self,
        repo_root: &Path,
        path: &Path,
        branch: &str,
        base_commit: &str,
    ) -> Result<(), GitError>;

    fn changes_since(
        &self,
        _worktree: &Path,
        _base_commit: &str,
    ) -> Result<Vec<GitChange>, GitError> {
        Ok(Vec::new())
    }

    fn cleanup_failed_worktree(&self, _repo_root: &Path, _path: &Path) -> Result<(), GitError> {
        Ok(())
    }

    fn remove_worktree(
        &self,
        _repo_root: &Path,
        _path: &Path,
        _branch: &str,
    ) -> Result<(), GitError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
pub struct SystemGit;

impl GitWorktrees for SystemGit {
    fn resolve_main(&self, repo_root: &Path) -> Result<String, GitError> {
        let output = git(
            repo_root,
            ["rev-parse", "--verify", "refs/heads/main^{commit}"],
        )?;
        success_text(output, "resolve local main")
    }

    fn create_worktree(
        &self,
        repo_root: &Path,
        path: &Path,
        branch: &str,
        base_commit: &str,
    ) -> Result<(), GitError> {
        let branch_check = git(repo_root, ["check-ref-format", "--branch", branch])?;
        success_text(branch_check, "validate branch")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let repo_root = git_compatible_path(repo_root);
        let path = git_compatible_path(path);
        let output = Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", repo_root.display()))
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "add", "--no-track", "-b"])
            .arg(branch)
            .arg(path)
            .arg(base_commit)
            .output()?;
        success_text(output, "create worktree").map(|_| ())
    }

    fn changes_since(
        &self,
        worktree: &Path,
        base_commit: &str,
    ) -> Result<Vec<GitChange>, GitError> {
        let worktree = git_compatible_path(worktree);
        let tracked = Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", worktree.display()))
            .arg("-C")
            .arg(&worktree)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--numstat",
                "-z",
            ])
            .arg(base_commit)
            .arg("--")
            .output()?;
        ensure_success(&tracked, "inspect tracked changes")?;

        let untracked = Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", worktree.display()))
            .arg("-C")
            .arg(&worktree)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()?;
        ensure_success(&untracked, "inspect untracked changes")?;

        let mut changes = parse_numstat(&tracked.stdout);
        for raw_path in untracked.stdout.split(|byte| *byte == 0) {
            let Some(file_key) = safe_file_key(raw_path) else {
                continue;
            };
            let (lines_added, binary) = untracked_file_stats(&worktree.join(&file_key))?;
            changes.push(GitChange {
                file_key,
                lines_added,
                lines_deleted: Some(0),
                binary,
                untracked: true,
            });
        }
        changes.sort_by(|left, right| left.file_key.cmp(&right.file_key));
        changes.dedup_by(|left, right| left.file_key == right.file_key);
        Ok(changes)
    }

    fn cleanup_failed_worktree(&self, repo_root: &Path, path: &Path) -> Result<(), GitError> {
        if !path.exists() {
            return Ok(());
        }
        let repo_root = git_compatible_path(repo_root);
        let path = git_compatible_path(path);
        let output = Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", repo_root.display()))
            .arg("-C")
            .arg(&repo_root)
            .args(["worktree", "remove", "--force"])
            .arg(&path)
            .output()?;
        ensure_success(&output, "clean failed worktree path")
    }

    fn remove_worktree(&self, repo_root: &Path, path: &Path, branch: &str) -> Result<(), GitError> {
        let branch_check = git(repo_root, ["check-ref-format", "--branch", branch])?;
        success_text(branch_check, "validate rollback branch")?;
        let repo_root = git_compatible_path(repo_root);
        let path = git_compatible_path(path);
        let mut first_error = None;
        if path.exists() {
            let output = Command::new("git")
                .arg("-c")
                .arg(format!("safe.directory={}", repo_root.display()))
                .arg("-C")
                .arg(&repo_root)
                .args(["worktree", "remove", "--force"])
                .arg(&path)
                .output()?;
            if let Err(error) = ensure_success(&output, "remove rolled-back worktree") {
                first_error = Some(error);
            }
        }
        let branch_output = Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", repo_root.display()))
            .arg("-C")
            .arg(&repo_root)
            .args(["branch", "-D", "--"])
            .arg(branch)
            .output()?;
        if let Err(error) = ensure_success(&branch_output, "remove rolled-back branch") {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn parse_numstat(output: &[u8]) -> Vec<GitChange> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let first_tab = entry.iter().position(|byte| *byte == b'\t')?;
            let second_tab = entry[first_tab + 1..]
                .iter()
                .position(|byte| *byte == b'\t')?
                + first_tab
                + 1;
            let added = &entry[..first_tab];
            let deleted = &entry[first_tab + 1..second_tab];
            let file_key = safe_file_key(&entry[second_tab + 1..])?;
            let binary = added == b"-" || deleted == b"-";
            Some(GitChange {
                file_key,
                lines_added: (!binary)
                    .then(|| std::str::from_utf8(added).ok()?.parse().ok())
                    .flatten(),
                lines_deleted: (!binary)
                    .then(|| std::str::from_utf8(deleted).ok()?.parse().ok())
                    .flatten(),
                binary,
                untracked: false,
            })
        })
        .collect()
}

fn safe_file_key(raw_path: &[u8]) -> Option<String> {
    let normalized = std::str::from_utf8(raw_path).ok()?.replace('\\', "/");
    let path = Path::new(&normalized);
    let file_name = normalized.rsplit('/').next()?.to_ascii_lowercase();
    let secret_name = file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name == "id_rsa"
        || file_name == "id_ed25519"
        || file_name.starts_with("id_");
    if normalized.is_empty()
        || path.is_absolute()
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "..")
        || normalized.chars().any(char::is_control)
        || secret_name
    {
        return None;
    }
    Some(normalized)
}

fn untracked_file_stats(path: &Path) -> Result<(Option<u64>, bool), GitError> {
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; 8192];
    let mut bytes = 0_u64;
    let mut newlines = 0_u64;
    let mut last = None;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if buffer[..read].contains(&0) {
            return Ok((None, true));
        }
        bytes = bytes.saturating_add(read as u64);
        newlines = newlines
            .saturating_add(buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64);
        last = buffer.get(read - 1).copied();
    }
    let lines = newlines.saturating_add(u64::from(bytes > 0 && last != Some(b'\n')));
    Ok((Some(lines), false))
}

fn ensure_success(output: &Output, operation: &str) -> Result<(), GitError> {
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(GitError::Command(format!("{operation} failed: {detail}")))
}

fn git<'a>(repo_root: &Path, args: impl IntoIterator<Item = &'a str>) -> Result<Output, GitError> {
    let repo_root = git_compatible_path(repo_root);
    Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", repo_root.display()))
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(GitError::from)
}

/// Git for Windows does not accept Win32 verbatim (`\\?\`) paths for a worktree's `.git`
/// indirection file. Keep canonical paths in the model, but remove the prefix at this process
/// boundary. UNC verbatim paths need their conventional leading double backslash restored.
fn git_compatible_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    value
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn success_text(output: Output, operation: &str) -> Result<String, GitError> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(GitError::Command(format!("{operation} failed: {detail}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn run(command: &mut Command) {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn temp_repo() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "optimus-git-worktrees-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        run(Command::new("git").args(["init", "-b", "main"]).arg(&path));
        run(Command::new("git").args(["-C"]).arg(&path).args([
            "config",
            "user.email",
            "optimus-tests@example.invalid",
        ]));
        run(Command::new("git").args(["-C"]).arg(&path).args([
            "config",
            "user.name",
            "Optimus Tests",
        ]));
        std::fs::write(path.join("README.md"), "base\n").unwrap();
        run(Command::new("git")
            .args(["-C"])
            .arg(&path)
            .args(["add", "README.md"]));
        run(Command::new("git")
            .args(["-C"])
            .arg(&path)
            .args(["commit", "-m", "base"]));
        path.canonicalize().unwrap()
    }

    #[test]
    fn real_sibling_worktrees_share_the_pinned_commit_without_moving_main() {
        let repo = temp_repo();
        let adapter = SystemGit;
        let base = adapter.resolve_main(&repo).unwrap();
        let one = repo.join(".worktrees/optimus/test/one");
        let two = repo.join(".worktrees/optimus/test/two");
        adapter
            .create_worktree(&repo, &one, "feat/test-one", &base)
            .unwrap();

        std::fs::write(repo.join("README.md"), "base\nmain moved\n").unwrap();
        run(Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["add", "README.md"]));
        run(Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["commit", "-m", "move main"]));
        let moved_main = adapter.resolve_main(&repo).unwrap();
        assert_ne!(base, moved_main);

        adapter
            .create_worktree(&repo, &two, "feat/test-two", &base)
            .unwrap();
        let one_head = success_text(git(&one, ["rev-parse", "HEAD"]).unwrap(), "head").unwrap();
        let two_head = success_text(git(&two, ["rev-parse", "HEAD"]).unwrap(), "head").unwrap();
        assert_eq!(base, one_head);
        assert_eq!(base, two_head);
        assert_eq!(moved_main, adapter.resolve_main(&repo).unwrap());

        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn change_capture_is_structured_private_and_ignores_diff_helpers() {
        let repo = temp_repo();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.join(".gitattributes"), "hostile.txt diff=hostile\n").unwrap();
        std::fs::write(repo.join("deleted.txt"), "remove me\n").unwrap();
        std::fs::write(repo.join("rename-old.txt"), "renamed\n").unwrap();
        std::fs::write(repo.join("hostile.txt"), "base\n").unwrap();
        std::fs::write(repo.join(".env"), "TOKEN=base\n").unwrap();
        std::fs::write(repo.join("private.pem"), "base\n").unwrap();
        std::fs::write(repo.join("image.bin"), [0_u8, 1, 2]).unwrap();
        run(Command::new("git").args(["-C"]).arg(&repo).args([
            "add",
            ".gitignore",
            ".gitattributes",
            "deleted.txt",
            "rename-old.txt",
            "hostile.txt",
            ".env",
            "private.pem",
            "image.bin",
        ]));
        run(Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["commit", "-m", "capture fixture"]));
        let adapter = SystemGit;
        let base = adapter.resolve_main(&repo).unwrap();

        run(Command::new("git").args(["-C"]).arg(&repo).args([
            "config",
            "diff.external",
            "optimus-command-that-must-not-run",
        ]));
        run(Command::new("git").args(["-C"]).arg(&repo).args([
            "config",
            "diff.hostile.textconv",
            "optimus-command-that-must-not-run",
        ]));
        std::fs::write(repo.join("README.md"), "base\nchanged\n").unwrap();
        std::fs::remove_file(repo.join("deleted.txt")).unwrap();
        std::fs::rename(repo.join("rename-old.txt"), repo.join("rename-new.txt")).unwrap();
        std::fs::write(repo.join("hostile.txt"), "changed\n").unwrap();
        std::fs::write(repo.join(".env"), "TOKEN=TOP_SECRET_SENTINEL\n").unwrap();
        std::fs::write(repo.join("private.pem"), "TOP_SECRET_SENTINEL\n").unwrap();
        std::fs::write(repo.join("image.bin"), [0_u8, 1, 3]).unwrap();
        std::fs::write(
            repo.join("untracked.txt"),
            "one\ntwo\nTOP_SECRET_SENTINEL\n",
        )
        .unwrap();
        std::fs::write(repo.join("untracked.bin"), [0_u8, 9, 8]).unwrap();
        std::fs::write(repo.join("ignored.txt"), "ignored\n").unwrap();
        std::fs::write(repo.join(".env.local"), "TOKEN=TOP_SECRET_SENTINEL\n").unwrap();
        std::fs::write(repo.join("id_ed25519"), "TOP_SECRET_SENTINEL\n").unwrap();

        let changes = adapter.changes_since(&repo, &base).unwrap();
        let change = |path: &str| {
            changes
                .iter()
                .find(|change| change.file_key == path)
                .unwrap_or_else(|| panic!("missing {path}: {changes:?}"))
        };
        assert_eq!(Some(1), change("README.md").lines_added);
        assert_eq!(Some(1), change("deleted.txt").lines_deleted);
        assert_eq!(Some(1), change("rename-old.txt").lines_deleted);
        assert!(change("rename-new.txt").untracked);
        assert_eq!(Some(3), change("untracked.txt").lines_added);
        assert!(change("image.bin").binary);
        assert!(change("untracked.bin").binary);
        assert!(change("hostile.txt").lines_added.is_some());
        for private in [
            ".env",
            ".env.local",
            "private.pem",
            "id_ed25519",
            "ignored.txt",
        ] {
            assert!(
                changes.iter().all(|change| change.file_key != private),
                "private or ignored path escaped: {private}"
            );
        }
        assert!(!format!("{changes:?}").contains("TOP_SECRET_SENTINEL"));

        std::fs::remove_dir_all(repo).unwrap();
    }
}
