//! Worktree Manager (impl plan §5.6, §F1.6; architecture §3.1, §3.7).
//!
//! A small, synchronous wrapper around the `git worktree` CLI. Each task gets
//! its own branch + worktree forked from the source repo's current `HEAD`:
//!
//! ```text
//! {worktree_root}/{task_id}/        (the working directory)
//! branch: divan/{task_id}-{slug}
//! ```
//!
//! Design rules enforced here:
//!
//! - Merge is NEVER automatic; it only happens via an explicit call (§5.6).
//! - `cleanup` removes ONLY the worktree (then prunes). It must not delete
//!   artifacts or the branch's committed history (§3.7). Artifact reference
//!   counting / `divan gc` is out of scope (v2).
//! - `is_dirty` lets the daemon warn before creating a worktree on a dirty
//!   source repo (§5.6); it does not block.
//!
//! This module is intentionally synchronous: `git` is invoked through
//! `std::process::Command`, which is blocking. Callers that need async should
//! offload to `tokio::task::spawn_blocking`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Errors produced by the worktree manager (impl plan §F1.6).
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// A `git` invocation exited non-zero. Carries a human-readable message
    /// (command + captured stderr) for trace logging (§5.6: errors -> trace).
    #[error("git: {0}")]
    Git(String),
    /// Failed to spawn `git` or otherwise touch the filesystem.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The source repo has uncommitted changes where a clean tree is required.
    #[error("dirty repo: {0}")]
    DirtyRepo(String),
    /// A referenced worktree / branch / path does not exist.
    #[error("not found: {0}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, WorktreeError>;

/// A task-scoped git worktree (impl plan §5.6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Worktree {
    /// Owning task id.
    pub task_id: String,
    /// Branch name: `divan/{task_id}-{slug}`.
    pub branch: String,
    /// Worktree directory: `{worktree_root}/{task_id}`.
    pub path: PathBuf,
}

/// Creates and tears down task worktrees over the `git` CLI (impl plan §5.6).
pub struct WorktreeManager {
    worktree_root: PathBuf,
}

impl WorktreeManager {
    /// Build a manager rooted at `worktree_root` (e.g. `<repo>/.divan/worktrees`).
    /// The directory is created lazily on `create`.
    pub fn new(worktree_root: impl Into<PathBuf>) -> Self {
        Self {
            worktree_root: worktree_root.into(),
        }
    }

    /// Branch name for a task: `divan/{task_id}-{slug}` (impl plan §5.6).
    fn branch_name(task_id: &str, slug: &str) -> String {
        format!("divan/{task_id}-{slug}")
    }

    /// Worktree directory for a task: `{worktree_root}/{task_id}`.
    fn worktree_path(&self, task_id: &str) -> PathBuf {
        self.worktree_root.join(task_id)
    }

    /// Run `git -C <repo> <args...>` with a deterministic, noise-free
    /// environment: no pager, no colour, fixed locale. Returns the captured
    /// `Output` on success (exit 0); maps non-zero exits to `WorktreeError::Git`.
    fn git(repo: &Path, args: &[&str]) -> Result<Output> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(repo).args(args);
        // Deterministic output: disable pager / colours / locale-dependent text.
        cmd.env("GIT_PAGER", "cat")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("NO_COLOR", "1");

        let output = cmd.output()?;
        if output.status.success() {
            Ok(output)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(WorktreeError::Git(format!(
                "`git {}` failed ({}): {}{}",
                args.join(" "),
                output.status,
                stderr.trim(),
                if stdout.trim().is_empty() {
                    String::new()
                } else {
                    format!(" | {}", stdout.trim())
                }
            )))
        }
    }

    /// `true` when `<path>` is a git work tree (i.e. `git rev-parse` succeeds).
    fn is_git_repo(repo: &Path) -> bool {
        Self::git(repo, &["rev-parse", "--is-inside-work-tree"]).is_ok()
    }

    /// `true` when `branch` already exists in `repo`.
    fn branch_exists(repo: &Path, branch: &str) -> bool {
        // `--verify --quiet` exits 0 iff the ref resolves.
        Self::git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }

    /// True if `repo`'s working tree has uncommitted changes (impl plan §5.6).
    ///
    /// Uses `git status --porcelain`, whose empty output is the stable signal
    /// for a clean tree. This is a *check only*: the daemon decides whether to
    /// warn; we never block worktree creation on it.
    pub fn is_dirty(&self, repo: &Path) -> Result<bool> {
        if !Self::is_git_repo(repo) {
            return Err(WorktreeError::NotFound(format!(
                "not a git repo: {}",
                repo.display()
            )));
        }
        let out = Self::git(repo, &["status", "--porcelain"])?;
        Ok(!out.stdout.is_empty())
    }

    /// Create a task worktree off `repo`'s current `HEAD` on a fresh branch
    /// (impl plan §5.6, §F1.6).
    ///
    /// Branch: `divan/{task_id}-{slug}`. Path: `{worktree_root}/{task_id}`.
    /// Fails clearly if `repo` is not a git repo or the branch already exists.
    pub fn create(&self, repo: &Path, task_id: &str, slug: &str) -> Result<Worktree> {
        if !Self::is_git_repo(repo) {
            return Err(WorktreeError::NotFound(format!(
                "not a git repo: {}",
                repo.display()
            )));
        }

        let branch = Self::branch_name(task_id, slug);
        if Self::branch_exists(repo, &branch) {
            return Err(WorktreeError::Git(format!(
                "branch already exists: {branch}"
            )));
        }

        let path = self.worktree_path(task_id);
        if path.exists() {
            return Err(WorktreeError::Git(format!(
                "worktree path already exists: {}",
                path.display()
            )));
        }

        // Ensure the parent (`worktree_root`) exists; `git worktree add` will
        // create the leaf directory itself.
        std::fs::create_dir_all(&self.worktree_root)?;

        // `git worktree add -b <branch> <path> HEAD`: new branch off current
        // HEAD, checked out at <path>.
        let path_str = path.to_string_lossy();
        Self::git(repo, &["worktree", "add", "-b", &branch, &path_str, "HEAD"])?;

        Ok(Worktree {
            task_id: task_id.to_string(),
            branch,
            path,
        })
    }

    /// Stage and commit ALL changes (including untracked files) in the worktree
    /// onto its branch.
    ///
    /// Coding agents edit files in the worktree but rarely run `git commit`, so
    /// without this the three-dot diff and `merge` would see nothing. Returns
    /// `Ok(())` with no commit when the tree is already clean (the agent made no
    /// changes). An explicit committer identity is supplied so the commit
    /// succeeds even if the host has no global `user.name`/`user.email`.
    pub fn commit_all(&self, wt: &Worktree, message: &str) -> Result<()> {
        let path = wt.path.as_path();
        if !Self::is_git_repo(path) {
            return Err(WorktreeError::NotFound(format!(
                "not a git worktree: {}",
                path.display()
            )));
        }
        Self::git(path, &["add", "-A"])?;
        // Nothing staged => the agent changed nothing; don't make an empty commit.
        let staged = Self::git(path, &["diff", "--cached", "--name-only"])?;
        if staged.stdout.is_empty() {
            return Ok(());
        }
        Self::git(
            path,
            &[
                "-c",
                "user.name=Divan",
                "-c",
                "user.email=divan@local",
                "commit",
                "--no-verify",
                "-m",
                message,
            ],
        )?;
        Ok(())
    }

    /// Unified diff of the worktree branch vs. the base it forked from
    /// (impl plan §F1.6: deterministic diff artifact).
    ///
    /// We diff `merge-base(<branch>, HEAD)...<branch>` using the three-dot form,
    /// which compares the branch tip against the common ancestor (the base HEAD
    /// the worktree forked from). This yields the *new* content introduced on
    /// the branch and is stable regardless of later movement on the base
    /// branch. Output is plain unified diff: no colour, no pager (see `git`).
    pub fn diff(&self, repo: &Path, wt: &Worktree) -> Result<String> {
        if !Self::is_git_repo(repo) {
            return Err(WorktreeError::NotFound(format!(
                "not a git repo: {}",
                repo.display()
            )));
        }
        if !Self::branch_exists(repo, &wt.branch) {
            return Err(WorktreeError::NotFound(format!(
                "worktree branch not found: {}",
                wt.branch
            )));
        }

        let range = format!("HEAD...{}", wt.branch);
        let out = Self::git(repo, &["diff", "--no-color", &range])?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Merge the worktree branch back into `repo`'s current branch.
    ///
    /// NEVER called automatically — only via an explicit user command
    /// (impl plan §5.6, §F1.6 acceptance). Refuses to merge into a dirty tree.
    pub fn merge(&self, repo: &Path, wt: &Worktree) -> Result<()> {
        if !Self::is_git_repo(repo) {
            return Err(WorktreeError::NotFound(format!(
                "not a git repo: {}",
                repo.display()
            )));
        }
        if !Self::branch_exists(repo, &wt.branch) {
            return Err(WorktreeError::NotFound(format!(
                "worktree branch not found: {}",
                wt.branch
            )));
        }
        // Merging into a dirty working tree risks clobbering local changes.
        if self.is_dirty(repo)? {
            return Err(WorktreeError::DirtyRepo(format!(
                "refusing to merge into dirty repo: {}",
                repo.display()
            )));
        }

        // `--no-edit` keeps it non-interactive and deterministic.
        Self::git(repo, &["merge", "--no-edit", &wt.branch])?;
        Ok(())
    }

    /// Remove ONLY the worktree, then prune stale administrative entries
    /// (impl plan §5.6; architecture §3.7).
    ///
    /// This deliberately does NOT delete artifacts, nor the branch's committed
    /// history — only the worktree directory is removed. `--force` is used so a
    /// worktree with a dirty/locked state can still be torn down; the branch
    /// and its commits survive in the repo's object store.
    pub fn cleanup(&self, repo: &Path, wt: &Worktree) -> Result<()> {
        if !Self::is_git_repo(repo) {
            return Err(WorktreeError::NotFound(format!(
                "not a git repo: {}",
                repo.display()
            )));
        }

        // If the directory is already gone, just prune and report success
        // (idempotent cleanup).
        if wt.path.exists() {
            let path_str = wt.path.to_string_lossy();
            Self::git(repo, &["worktree", "remove", "--force", &path_str])?;
        }
        // Prune dangling worktree metadata regardless. NOTE: no artifact or
        // branch deletion here (§3.7).
        Self::git(repo, &["worktree", "prune"])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// True if `git` is on PATH. Tests skip (return early) when it is not.
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Run a raw `git -C <repo> <args>` in a test, panicking on failure.
    fn git_ok(repo: &Path, args: &[&str]) {
        let out = WorktreeManager::git(repo, args)
            .unwrap_or_else(|e| panic!("git {:?} failed: {e}", args));
        assert!(out.status.success());
    }

    /// Init a temp git repo with one committed file and return its path.
    fn init_repo(dir: &Path) {
        git_ok(dir, &["init", "-b", "main"]);
        git_ok(dir, &["config", "user.email", "test@divan.local"]);
        git_ok(dir, &["config", "user.name", "Divan Test"]);
        std::fs::write(dir.join("README.md"), "hello divan\n").unwrap();
        git_ok(dir, &["add", "README.md"]);
        git_ok(dir, &["commit", "-m", "initial"]);
    }

    #[test]
    fn create_makes_branch_and_dir_with_expected_naming() {
        if !git_available() {
            eprintln!("skipping: git not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        let wt = mgr.create(&repo, "t-123", "add-feature").unwrap();

        assert_eq!(wt.task_id, "t-123");
        assert_eq!(wt.branch, "divan/t-123-add-feature");
        assert_eq!(wt.path, tmp.path().join("worktrees").join("t-123"));
        assert!(wt.path.is_dir(), "worktree dir should exist");

        // The branch must actually exist in the repo.
        assert!(WorktreeManager::branch_exists(&repo, &wt.branch));
        // The checked-out file from HEAD should be present in the worktree.
        assert!(wt.path.join("README.md").exists());
    }

    #[test]
    fn commit_all_captures_uncommitted_edits_so_diff_is_nonempty() {
        // Regression: agents edit the worktree without committing; without
        // commit_all the three-dot diff is empty (F1.6/F1.10).
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let mgr = WorktreeManager::new(tmp.path().join("wt"));
        let wt = mgr.create(&repo, "t-1", "feat").unwrap();

        // Simulate an agent creating an untracked file (no commit).
        std::fs::write(wt.path.join("feature.txt"), "agent edit\n").unwrap();
        assert_eq!(
            mgr.diff(&repo, &wt).unwrap(),
            "",
            "uncommitted => empty diff"
        );

        // commit_all snapshots it; now the diff has content.
        mgr.commit_all(&wt, "divan: implement").unwrap();
        let diff = mgr.diff(&repo, &wt).unwrap();
        assert!(diff.contains("feature.txt"), "diff names the new file");
        assert!(diff.contains("agent edit"), "diff contains the new content");

        // A second commit_all with no changes is a no-op (no empty commit).
        mgr.commit_all(&wt, "divan: noop").unwrap();
    }

    #[test]
    fn create_rejects_non_repo_and_duplicate_branch() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();

        // Not a git repo.
        let not_repo = tmp.path().join("plain");
        std::fs::create_dir_all(&not_repo).unwrap();
        let mgr = WorktreeManager::new(tmp.path().join("wt"));
        assert!(matches!(
            mgr.create(&not_repo, "t-1", "x"),
            Err(WorktreeError::NotFound(_))
        ));

        // Duplicate branch.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let _wt = mgr.create(&repo, "t-1", "x").unwrap();
        // Same task id -> same branch -> conflict (path also gone first; clean it).
        mgr.cleanup(&repo, &_wt).unwrap();
        assert!(matches!(
            mgr.create(&repo, "t-1", "x"),
            Err(WorktreeError::Git(_))
        ));
    }

    #[test]
    fn diff_contains_new_file_committed_in_worktree() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        let wt = mgr.create(&repo, "t-77", "work").unwrap();

        // Add + commit a new file *inside the worktree*.
        std::fs::write(wt.path.join("feature.txt"), "MAGIC_CONTENT_123\n").unwrap();
        git_ok(&wt.path, &["add", "feature.txt"]);
        git_ok(&wt.path, &["commit", "-m", "add feature"]);

        let diff = mgr.diff(&repo, &wt).unwrap();
        assert!(
            diff.contains("feature.txt"),
            "diff should name the new file:\n{diff}"
        );
        assert!(
            diff.contains("MAGIC_CONTENT_123"),
            "diff should include new content:\n{diff}"
        );
    }

    #[test]
    fn cleanup_removes_worktree_but_keeps_repo_and_history() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        let wt = mgr.create(&repo, "t-9", "cleanme").unwrap();

        // Commit something so the branch has history we expect to survive.
        std::fs::write(wt.path.join("a.txt"), "data\n").unwrap();
        git_ok(&wt.path, &["add", "a.txt"]);
        git_ok(&wt.path, &["commit", "-m", "c1"]);

        assert!(wt.path.exists());
        mgr.cleanup(&repo, &wt).unwrap();

        // Worktree dir is gone...
        assert!(!wt.path.exists(), "worktree dir must be removed");
        // ...but the source repo and its .git are intact (artifacts/history
        // untouched per §3.7).
        assert!(repo.join(".git").exists(), "repo .git must survive cleanup");
        assert!(repo.join("README.md").exists(), "repo files must survive");
        // The branch's commit object still resolves in the repo.
        assert!(WorktreeManager::branch_exists(&repo, &wt.branch));

        // Cleanup is idempotent: a second call must not error.
        mgr.cleanup(&repo, &wt).unwrap();
    }

    #[test]
    fn is_dirty_false_on_clean_true_after_change() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        assert!(!mgr.is_dirty(&repo).unwrap(), "fresh repo should be clean");

        std::fs::write(repo.join("README.md"), "changed\n").unwrap();
        assert!(
            mgr.is_dirty(&repo).unwrap(),
            "modified repo should be dirty"
        );
    }

    #[test]
    fn merge_brings_worktree_commit_into_base() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        let wt = mgr.create(&repo, "t-merge", "feat").unwrap();

        std::fs::write(wt.path.join("merged.txt"), "from-branch\n").unwrap();
        git_ok(&wt.path, &["add", "merged.txt"]);
        git_ok(&wt.path, &["commit", "-m", "branch work"]);

        // The file does not exist on the base checkout yet.
        assert!(!repo.join("merged.txt").exists());

        mgr.merge(&repo, &wt).unwrap();

        // After merge, the base working tree contains the branch's file.
        assert!(
            repo.join("merged.txt").exists(),
            "merge should bring the branch commit into base"
        );
    }

    #[test]
    fn merge_refuses_dirty_base() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let mgr = WorktreeManager::new(tmp.path().join("worktrees"));
        let wt = mgr.create(&repo, "t-dirty", "feat").unwrap();
        std::fs::write(wt.path.join("x.txt"), "x\n").unwrap();
        git_ok(&wt.path, &["add", "x.txt"]);
        git_ok(&wt.path, &["commit", "-m", "w"]);

        // Dirty the base working tree.
        std::fs::write(repo.join("README.md"), "dirty\n").unwrap();
        assert!(matches!(
            mgr.merge(&repo, &wt),
            Err(WorktreeError::DirtyRepo(_))
        ));
    }
}
