use std::path::{Path, PathBuf};
use std::process::Command;
use zmem_core::{AttentionPolicy, GitRepo, TrailIdentity};

#[test]
fn trail_identity_changes_with_every_compatibility_input() {
    let base = TrailIdentity::new(1, "a".repeat(40), AttentionPolicy::default(), "ext", 4, 4);
    assert_eq!(
        base,
        TrailIdentity::new(1, "a".repeat(40), AttentionPolicy::default(), "ext", 4, 4)
    );
    assert_ne!(
        base,
        TrailIdentity::new(1, "b".repeat(40), AttentionPolicy::default(), "ext", 4, 4)
    );
    assert_ne!(
        base,
        TrailIdentity::new(1, "a".repeat(40), AttentionPolicy::default(), "other", 4, 4)
    );
}

#[test]
fn route_identity_follows_named_refs_but_keeps_detached_expressions_separate() {
    let repo = TestRepo::new();
    let git = GitRepo::open(&repo.0).unwrap();
    let head = git.head().unwrap();
    for alias in ["HEAD", "main", "refs/heads/main"] {
        assert_eq!(
            git.resolve_observed(alias, &head).unwrap().route,
            "refs/heads/main"
        );
    }
    assert_eq!(
        git.resolve_observed("feature", &head).unwrap().route,
        "refs/heads/feature"
    );
    assert_eq!(
        git.resolve_observed(&head, &head).unwrap().route,
        format!("oid:{head}")
    );
    repo.advance_feature();
    let advanced = git.resolve("feature").unwrap();
    assert_eq!(
        git.resolve_observed("feature", &advanced).unwrap().route,
        "refs/heads/feature"
    );
    run(&repo.0, &["checkout", "-q", "--detach", &head]);
    assert_eq!(
        git.resolve_observed("HEAD", &head).unwrap().route,
        format!("oid:{head}")
    );
}

#[test]
fn live_resolution_rejects_a_stale_observation_without_checkout() {
    let repo = TestRepo::new();
    let git = GitRepo::open(&repo.0).unwrap();
    let observed = git.resolve("feature").unwrap();
    repo.advance_feature();
    let before = git.head().unwrap();
    let error = git.resolve_observed("feature", &observed).unwrap_err();
    assert!(error.to_string().contains("stale ref"));
    assert_eq!(git.head().unwrap(), before);
}

struct TestRepo(PathBuf);

impl TestRepo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zmem-trails-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        run(&path, &["init", "-q", "-b", "main"]);
        run(&path, &["config", "user.name", "Test"]);
        run(&path, &["config", "user.email", "test@example.com"]);
        std::fs::write(path.join("file"), "one").unwrap();
        run(&path, &["add", "file"]);
        run(&path, &["commit", "-q", "-m", "one"]);
        run(&path, &["branch", "feature"]);
        Self(path)
    }

    fn advance_feature(&self) {
        run(&self.0, &["checkout", "-q", "feature"]);
        std::fs::write(self.0.join("file"), "two").unwrap();
        run(&self.0, &["add", "file"]);
        run(&self.0, &["commit", "-q", "-m", "two"]);
        run(&self.0, &["checkout", "-q", "main"]);
    }
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
