use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use zmem_store::Store;
use zmem_svc::{PrefetchSession, PrefetchTurn};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zmem-prefetch-turns-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn each_prefetch_turn_commits_at_most_64_facts_and_keeps_its_checkpoint() {
    let fixture = TestDir::new();
    let repo = fixture.0.join("repo");
    let home = fixture.0.join("home");
    std::fs::create_dir_all(&home).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for index in 0..130 {
        let body = format!("commit {index}\n");
        write!(
            input,
            "commit refs/heads/main\nmark :{}\nauthor Test <test@example.com> 1700000000 +0000\ncommitter Test <test@example.com> 1700000000 +0000\ndata {}\n{}",
            index + 1,
            body.len(),
            body
        )
        .unwrap();
        if index > 0 {
            writeln!(input, "from :{index}").unwrap();
        }
        writeln!(input, "M 100644 inline file.txt\ndata 1\nx\n").unwrap();
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    let head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();

    let database = home.join("db").join("entries.db");
    let mut store = Store::open(&database).unwrap();
    let canonical = zmem_core::GitRepo::open(&repo).unwrap();
    let repo_id = store
        .register_repository(&canonical.root().to_string_lossy(), false)
        .unwrap();
    drop(store);
    // This integration binary contains one test, so its temporary home cannot
    // race another test's process environment.
    unsafe { std::env::set_var("ZMEM_HOME", &home) };
    let stopped = AtomicBool::new(false);
    let demand = AtomicUsize::new(0);
    let completed_foreground = AtomicUsize::new(0);
    let mut session = PrefetchSession::start(&repo, &head).unwrap().unwrap();
    // Foreground demand stays ready throughout the background turns. Each
    // turn must still advance a bounded checkpoint instead of starving.
    demand.store(1, Ordering::Release);
    for expected in [64, 128] {
        let started = std::time::Instant::now();
        assert_eq!(
            session
                .advance(&stopped, &demand, &completed_foreground)
                .unwrap(),
            PrefetchTurn::More
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(
            Store::open_readonly(&database)
                .unwrap()
                .prefetch_checkpoint(repo_id, &head)
                .unwrap()
                .unwrap()
                .0,
            expected
        );
    }
    assert_eq!(
        session
            .advance(&stopped, &demand, &completed_foreground)
            .unwrap(),
        PrefetchTurn::Done
    );
    demand.store(0, Ordering::Release);
    assert_eq!(
        Store::open_readonly(&database)
            .unwrap()
            .prefetch_checkpoint(repo_id, &head)
            .unwrap()
            .unwrap()
            .0,
        130
    );
    drop(session);
    // An obsolete checkpoint must not skip facts removed by reclamation.
    let mut store = Store::open_writable_existing(&database).unwrap();
    store
        .set_prefetch_state(repo_id, &head, "obsolete")
        .unwrap();
    assert!(store.raw_commit(repo_id, &head).unwrap().is_none());
    let mut resumed =
        PrefetchSession::start_target(&repo, &head, 64, vec!["refs/heads/main".into()])
            .unwrap()
            .unwrap();
    assert_eq!(
        resumed
            .advance(&stopped, &demand, &completed_foreground)
            .unwrap(),
        PrefetchTurn::Done
    );
    assert_eq!(
        store
            .prefetch_checkpoint(repo_id, &head)
            .unwrap()
            .unwrap()
            .0,
        64
    );
    drop(resumed);
    let mut extended =
        PrefetchSession::start_target(&repo, &head, 130, vec!["refs/heads/main".into()])
            .unwrap()
            .unwrap();
    assert_eq!(
        extended
            .advance(&stopped, &demand, &completed_foreground)
            .unwrap(),
        PrefetchTurn::More
    );
    assert_eq!(
        store
            .prefetch_checkpoint(repo_id, &head)
            .unwrap()
            .unwrap()
            .0,
        128
    );
    // Named refs remain eligible even when the worktree branch moves.
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "other", &head])
            .status()
            .unwrap()
            .success()
    );
    let parent = zmem_core::GitRepo::open(&repo)
        .unwrap()
        .resolve("HEAD^")
        .unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["update-ref", "refs/heads/main", &parent])
            .status()
            .unwrap()
            .success()
    );
    extended.set_references(vec!["refs/heads/other".into()]);
    assert_eq!(
        extended
            .advance(&stopped, &demand, &completed_foreground)
            .unwrap(),
        PrefetchTurn::Done
    );
    assert_eq!(
        store
            .prefetch_checkpoint(repo_id, &head)
            .unwrap()
            .unwrap()
            .0,
        130
    );
    drop(extended);
    store
        .set_prefetch_state(repo_id, &head, "obsolete")
        .unwrap();
    let mut shared = PrefetchSession::start_target(
        &repo,
        &head,
        130,
        vec!["refs/heads/main".into(), "refs/heads/other".into()],
    )
    .unwrap()
    .unwrap();
    // The moved high-demand ref must not lend its target to the surviving
    // lower-demand alias, even though collection is shared by pinned OID.
    shared.set_route_targets(vec![
        ("refs/heads/main".into(), 130),
        ("refs/heads/other".into(), 64),
    ]);
    assert_eq!(
        shared
            .advance(&stopped, &demand, &completed_foreground)
            .unwrap(),
        PrefetchTurn::Done
    );
    assert_eq!(
        store
            .prefetch_checkpoint(repo_id, &head)
            .unwrap()
            .unwrap()
            .0,
        64
    );
    drop(shared);
    unsafe { std::env::remove_var("ZMEM_HOME") };
}
