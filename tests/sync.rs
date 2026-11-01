use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const AGENT_ENV: &[&str] = &["HIPPO_SOURCE", "PI_CODING_AGENT", "CURSOR_AGENT"];

fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("hippo-sync-{nanos}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A data repo whose `main` tracks a local bare remote.
fn data_repo() -> (PathBuf, PathBuf) {
    let root = temp_dir();
    let remote = root.join("remote.git");
    let home = root.join("home");
    git(
        &root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(&root, &["init", "-q", "-b", "main", home.to_str().unwrap()]);
    for (key, value) in [
        ("user.name", "Hippo Test"),
        ("user.email", "hippo@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&home, &["config", key, value]);
    }
    fs::write(home.join(".gitignore"), ".index/\n").unwrap();
    git(&home, &["add", ".gitignore"]);
    git(&home, &["commit", "-q", "-m", "init"]);
    git(
        &home,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&home, &["push", "-q", "-u", "origin", "main"]);
    (home, remote)
}

fn hippo(home: &Path, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hippo"));
    cmd.arg("--home").arg(home).args(args);
    for name in AGENT_ENV {
        cmd.env_remove(name);
    }
    cmd.envs(env.iter().copied());
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn add(home: &Path, title: &str, env: &[(&str, &str)]) -> Output {
    hippo(
        home,
        &["add", "--title", title, "--keywords", "sync,test", "--json"],
        Some("A durable fact that is long enough to pass the capture checks.\n"),
        env,
    )
}

fn ok(output: &Output) -> serde_json::Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap_or_default()
}

fn remote_files(remote: &Path) -> String {
    git(remote, &["ls-tree", "-r", "--name-only", "main"])
}

fn assert_synced(home: &Path) {
    assert_eq!(git(home, &["status", "--porcelain"]), "");
    assert_eq!(
        git(home, &["rev-list", "--count", "@{u}..HEAD"]).trim(),
        "0"
    );
}

#[test]
fn add_commits_and_pushes() {
    let (home, remote) = data_repo();
    let payload = ok(&add(&home, "Sync on add", &[]));
    assert!(payload.get("warnings").is_none(), "{payload}");
    assert!(remote_files(&remote).contains("sync-on-add.md"));
    assert_eq!(
        git(&remote, &["log", "-1", "--format=%s", "main"]).trim(),
        "Record memory updates."
    );
    assert_synced(&home);
}

#[test]
fn update_and_consolidate_apply_commit_and_push() {
    let (home, remote) = data_repo();
    ok(&add(&home, "Sync on update", &[]));
    ok(&hippo(
        &home,
        &[
            "update",
            "sync-on-update",
            "--body",
            "A changed body that is still long enough to keep.",
        ],
        None,
        &[],
    ));
    assert!(
        git(&remote, &["show", "main:memory/general/sync-on-update.md"]).contains("A changed body")
    );

    ok(&add(&home, "Duplicate fact", &[]));
    ok(&add(&home, "Duplicate fact again", &[]));
    let report = ok(&hippo(
        &home,
        &["consolidate", "--apply", "--json"],
        None,
        &[],
    ));
    let removed = report["removed"].as_array().unwrap();
    assert!(!removed.is_empty(), "{report}");
    let files = remote_files(&remote);
    for id in removed {
        assert!(
            !files.contains(&format!("{}.md", id.as_str().unwrap())),
            "{files}"
        );
    }
    assert_synced(&home);
}

#[test]
fn sync_commits_a_hand_edit_and_skips_other_files() {
    let (home, remote) = data_repo();
    ok(&add(&home, "Hand edited", &[]));
    let file = home.join("memory/general/hand-edited.md");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}\nAn extra line typed by hand.\n")).unwrap();

    let outside = home.join("notes.md");
    fs::write(&outside, "not a memory\n").unwrap();
    let skipped = hippo(
        &home,
        &["sync", "--file", outside.to_str().unwrap()],
        None,
        &[],
    );
    assert!(String::from_utf8_lossy(&skipped.stdout).starts_with("skipped:"));
    assert!(!git(&home, &["status", "--porcelain"]).is_empty());

    let synced = hippo(
        &home,
        &["sync", "--file", file.to_str().unwrap()],
        None,
        &[],
    );
    ok(&synced);
    assert_eq!(
        String::from_utf8_lossy(&synced.stdout).trim(),
        "committed=true pushed=true"
    );
    assert!(
        git(&remote, &["show", "main:memory/general/hand-edited.md"]).contains("typed by hand")
    );
    let status: serde_json::Value = ok(&hippo(&home, &["status", "--json"], None, &[]));
    assert_eq!(status["changed"], serde_json::json!([]));
}

#[test]
fn failed_push_keeps_the_memory_and_warns() {
    let (home, remote) = data_repo();
    fs::rename(&remote, remote.with_extension("moved")).unwrap();
    let payload = ok(&add(&home, "Offline save", &[]));
    let warning = payload["warnings"][0].as_str().unwrap();
    assert!(
        warning.starts_with("memory saved but not synced: git push failed"),
        "{warning}"
    );
    assert!(home.join("memory/general/offline-save.md").is_file());
    assert_eq!(
        git(&home, &["rev-list", "--count", "@{u}..HEAD"]).trim(),
        "1"
    );
}

#[test]
fn home_outside_git_is_left_alone() {
    let home = temp_dir();
    let payload = ok(&add(&home, "Outside any repo", &[]));
    assert!(payload.get("warnings").is_none(), "{payload}");
    assert!(!home.join(".git").exists());
}

#[test]
fn parallel_writers_all_land() {
    let (home, remote) = data_repo();
    let writers: Vec<_> = (0..6)
        .map(|n| {
            let home = home.clone();
            std::thread::spawn(move || add(&home, &format!("Parallel writer {n}"), &[]))
        })
        .collect();
    for writer in writers {
        let payload = ok(&writer.join().unwrap());
        assert!(payload.get("warnings").is_none(), "{payload}");
    }
    let files = remote_files(&remote);
    for n in 0..6 {
        assert!(
            files.contains(&format!("parallel-writer-{n}.md")),
            "{files}"
        );
    }
    assert_synced(&home);
}

/// Title, environment, extra `add` flags, expected frontmatter line.
type SourceCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str], &'a str);

#[test]
fn source_comes_from_the_agent_environment() {
    let home = temp_dir();
    let cases: &[SourceCase] = &[
        (
            "Inside pi",
            &[("PI_CODING_AGENT", "true")],
            &[],
            "source: pi",
        ),
        (
            "Inside cursor",
            &[("CURSOR_AGENT", "1")],
            &[],
            "source: cursor",
        ),
        (
            "Named source",
            &[("HIPPO_SOURCE", "codex"), ("PI_CODING_AGENT", "true")],
            &[],
            "source: codex",
        ),
        (
            "Flag wins",
            &[("PI_CODING_AGENT", "true")],
            &["--source", "manual"],
            "source: manual",
        ),
    ];
    for (title, env, extra, expected) in cases {
        let mut args = vec![
            "add",
            "--title",
            title,
            "--keywords",
            "source,test",
            "--json",
        ];
        args.extend_from_slice(extra);
        let payload = ok(&hippo(
            &home,
            &args,
            Some("A durable fact that is long enough to pass the capture checks.\n"),
            env,
        ));
        let shown = hippo(&home, &["show", payload["id"].as_str().unwrap()], None, &[]);
        assert!(
            String::from_utf8_lossy(&shown.stdout).contains(expected),
            "{title}"
        );
    }
    let plain = ok(&add(&home, "No agent", &[]));
    let shown = hippo(&home, &["show", plain["id"].as_str().unwrap()], None, &[]);
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("source:"));
}
