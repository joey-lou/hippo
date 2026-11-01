use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

fn adapters() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters")
}

fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("hippo-hooks-{nanos}-{}-{n}", std::process::id()));
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
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Runs a Cursor hook with the built `hippo` first on PATH and `home` as the data home.
fn hook(script: &str, home: &Path, payload: &str, env: &[(&str, &str)]) -> Output {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_hippo")).parent().unwrap();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let mut child = Command::new("bash")
        .arg(adapters().join("cursor/hooks").join(script))
        .env("PATH", path)
        .env("HIPPO_HOME", home)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn data_repo() -> PathBuf {
    let home = temp_dir();
    git(&home, &["init", "-q", "-b", "main"]);
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
    home
}

#[test]
fn after_file_edit_commits_memory_edits_only() {
    let home = data_repo();
    let memory = home.join("memory/general/edited.md");
    fs::create_dir_all(memory.parent().unwrap()).unwrap();
    fs::write(
        &memory,
        "---\nid: edited\ntitle: Edited by hand\nkeywords: [edit]\ncreated: 2026-01-01\nupdated: 2026-01-01\nscope: global\n---\nA memory body edited directly in the editor.\n",
    )
    .unwrap();
    let other = home.join("scratch.md");
    fs::write(&other, "not a memory\n").unwrap();

    let payload = |path: &Path| serde_json::json!({ "file_path": path }).to_string();
    hook("hippo-after-file-edit.sh", &home, &payload(&other), &[]);
    assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]).trim(), "1");

    let output = hook("hippo-after-file-edit.sh", &home, &payload(&memory), &[]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(git(&home, &["status", "--porcelain"]), "?? scratch.md\n");
}

#[test]
fn session_start_injects_the_digest() {
    let home = temp_dir();
    for env in [&[][..], &[("PI_CODING_AGENT", "true")][..]] {
        let cursor = hook("hippo-session-start.sh", &home, "{}", env);
        let context: serde_json::Value = serde_json::from_slice(&cursor.stdout).unwrap();
        assert!(
            context["additional_context"]
                .as_str()
                .unwrap()
                .contains("What's in memory"),
            "{env:?}"
        );
    }
}

#[test]
fn both_adapters_ship_the_same_skill() {
    let read = |name: &str| {
        fs::read_to_string(adapters().join(name).join("skills/use-memory/SKILL.md")).unwrap()
    };
    assert_eq!(read("cursor"), read("pi"));
}
