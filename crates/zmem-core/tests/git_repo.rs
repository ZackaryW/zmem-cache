use std::path::{Path, PathBuf};
use std::process::Command;
use zmem_core::{AttentionLimit, GitRepo};

struct TestRepo(PathBuf);

impl TestRepo {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("zmem-core-git-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        run(&path, &["init", "-q"]);
        run(&path, &["config", "user.name", "Test"]);
        run(&path, &["config", "user.email", "test@example.com"]);
        std::fs::write(path.join("memory.txt"), "one").unwrap();
        run(&path, &["add", "memory.txt"]);
        run(&path, &["commit", "-q", "-m", "feat: one"]);
        Self(path)
    }

    fn commit(&self, content: &str) {
        std::fs::write(self.0.join("memory.txt"), content).unwrap();
        run(&self.0, &["add", "memory.txt"]);
        run(
            &self.0,
            &["commit", "-q", "-m", &format!("feat: {content}")],
        );
    }
}

#[test]
fn newest_walk_uses_a_sentinel_without_materializing_older_history() {
    let temporary = TestRepo::new();
    temporary.commit("two");
    temporary.commit("three");
    let repo = GitRepo::open(&temporary.0).unwrap();
    let head = repo.head().unwrap();

    let bounded = repo
        .walk_newest(&head, AttentionLimit::parse(2, "commit").unwrap())
        .unwrap();
    assert_eq!(bounded.shas.len(), 2);
    assert_eq!(bounded.shas[0], head);
    assert!(bounded.truncated);

    let complete = repo.walk_newest(&head, AttentionLimit::Unlimited).unwrap();
    assert_eq!(complete.shas.len(), 3);
    assert!(!complete.truncated);
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(path: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn references_resolve_to_full_commits_for_selected_replay() {
    let temporary = TestRepo::new();
    let repo = GitRepo::open(&temporary.0).unwrap();
    let head = repo.head().unwrap();
    assert_eq!(repo.resolve("HEAD").unwrap(), head);
    assert_eq!(repo.walk(None, &head).unwrap(), vec![head]);
}
