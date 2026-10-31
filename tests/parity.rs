//! Behavioral parity with the Python Hippo CLI.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use chrono::NaiveDate;
use hippo_cli::capture::{self, assess};
use hippo_cli::config::{self, GLOBAL_SCOPE};
use hippo_cli::embed::{self, DIM};
use hippo_cli::error::HippoError;
use hippo_cli::frontmatter;
use hippo_cli::hygiene::{self, analyze};
use hippo_cli::index;
use hippo_cli::ranking::{self, RECENCY_WEIGHT, USAGE_WEIGHT};
use hippo_cli::render;
use hippo_cli::search;
use hippo_cli::store::{self, Memory, MemoryPatch, NewMemory};
use hippo_cli::MemoryStore;

static ENV_LOCK: Mutex<()> = Mutex::new(());

const BODY: &str = "This preference stays true across sessions and projects.";
const TITLE: &str = "Durable preference note";

fn temp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hippo-cli-{nanos}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn fresh_home(seeded: bool) -> PathBuf {
    let dir = temp_dir();
    if seeded {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory");
        copy_tree(&fixture, &dir.join("memory"));
    } else {
        fs::create_dir_all(dir.join("memory")).unwrap();
    }
    dir
}

fn files(home: &Path) -> Vec<String> {
    let mut paths: Vec<String> = store::iter_memory_files(home)
        .iter()
        .map(|path| {
            path.strip_prefix(home)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    paths.sort();
    paths
}

fn draft(title: &str, keywords: &[&str], body: &str) -> NewMemory {
    NewMemory {
        title: title.into(),
        keywords: keywords.iter().map(|item| (*item).to_string()).collect(),
        body: body.into(),
        tags: Vec::new(),
        category: store::DEFAULT_CATEGORY.into(),
        scope: GLOBAL_SCOPE.into(),
        memory_id: None,
        source: None,
        reason: None,
        confidence: None,
    }
}

fn mem(id: &str, title: &str, keywords: &[&str], updated: &str, body: &str) -> Memory {
    Memory {
        id: id.into(),
        title: title.into(),
        keywords: keywords.iter().map(|item| (*item).to_string()).collect(),
        created: "2026-01-01".into(),
        updated: updated.into(),
        body: body.into(),
        tags: Vec::new(),
        scope: GLOBAL_SCOPE.into(),
        source: None,
        reason: None,
        confidence: None,
        path: PathBuf::new(),
        content_hash: String::new(),
    }
}

fn hippo(home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hippo"));
    cmd.arg("--home").arg(home).args(args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
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

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn write_ranked(home: &Path, memory_id: &str, updated: &str) {
    let folder = home.join("memory").join("coding");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(format!("{memory_id}.md")),
        format!(
            "---\nid: {memory_id}\ntitle: {memory_id}\nkeywords: [alpha, beta, gamma]\ncreated: 2026-01-01\nupdated: {updated}\nscope: global\n---\nbody about alpha, beta, gamma\n"
        ),
    )
    .unwrap();
}

#[test]
fn slugify_and_store_round_trip() {
    assert_eq!(store::slugify("Hello, World!").unwrap(), "hello-world");
    assert!(store::slugify("!!!").is_err());

    let home = fresh_home(false);
    let memory = store::write_new(
        &home,
        draft("Test Memory", &["alpha", "beta"], "Some body."),
    )
    .unwrap();
    assert_eq!(memory.id, "test-memory");
    assert!(memory.path.exists());
    assert!(!memory.content_hash.is_empty());
    let reread = store::read_memory(&memory.path).unwrap();
    assert_eq!(reread.title, "Test Memory");
    assert_eq!(reread.keywords, ["alpha", "beta"]);

    store::write_new(&home, draft("Dup", &["x"], "a")).unwrap();
    assert!(store::write_new(&home, draft("Dup", &["x"], "b")).is_err());

    let updated = store::update(
        &home,
        "editable-missing",
        MemoryPatch {
            title: None,
            keywords: None,
            tags: None,
            body: None,
            scope: None,
            reason: None,
            confidence: None,
        },
    );
    assert!(matches!(updated, Err(HippoError::NotFound(_))));

    store::write_new(&home, draft("Editable", &["x"], "v1")).unwrap();
    let updated = store::update(
        &home,
        "editable",
        MemoryPatch {
            title: None,
            keywords: Some(vec!["y".into(), "z".into()]),
            tags: None,
            body: Some("v2".into()),
            scope: None,
            reason: None,
            confidence: None,
        },
    )
    .unwrap();
    assert_eq!(updated.body.trim(), "v2");
    assert_eq!(updated.keywords, ["y", "z"]);

    assert!(matches!(
        store::find_path(&home, "nope"),
        Err(HippoError::NotFound(_))
    ));

    let mismatch = home.join("memory").join("wrong.md");
    fs::write(
        &mismatch,
        "---\nid: right\ntitle: t\nkeywords: [a]\ncreated: 2026-01-01\nupdated: 2026-01-01\n---\n\nbody\n",
    )
    .unwrap();
    assert!(matches!(
        store::read_memory(&mismatch),
        Err(HippoError::Validation(_))
    ));
}

#[test]
fn frontmatter_required_fields() {
    let valid = "---\nid: sample\ntitle: A sample\nkeywords: [a, b]\ncreated: 2026-01-01\nupdated: 2026-01-01\n---\n\nBody text here.\n";
    let (meta, body) = frontmatter::parse(valid).unwrap();
    assert_eq!(
        frontmatter::mapping_str(&meta, "id").as_deref(),
        Some("sample")
    );
    assert_eq!(body.trim(), "Body text here.");
    assert!(frontmatter::parse("no fence here").is_err());
    assert!(frontmatter::validate_fields("x", "t", &[], "d", "d", None, None).is_err());
    assert!(frontmatter::validate_fields("", "t", &["a".into()], "d", "d", None, None).is_err());
}

#[test]
fn config_resolution() {
    let home = fresh_home(false);
    assert_eq!(config::resolve_home(Some(&home)).unwrap(), home);
    let missing = home.join("absent");
    assert_eq!(config::resolve_home(Some(&missing)).unwrap(), missing);
    assert!(missing.is_dir());
    let file_home = home.join("a-file");
    fs::write(&file_home, "x").unwrap();
    assert!(matches!(
        config::resolve_home(Some(&file_home)),
        Err(HippoError::Config(_))
    ));

    let _guard = ENV_LOCK.lock().unwrap();
    let saved_user_home = std::env::var_os("HOME");
    let saved_home = std::env::var_os("HIPPO_HOME");
    let saved_scope = std::env::var_os("HIPPO_SCOPE");
    unsafe {
        std::env::set_var("HIPPO_HOME", &home);
        std::env::remove_var("HIPPO_SCOPE");
    }
    assert_eq!(config::resolve_home(None).unwrap(), home);
    assert_eq!(config::resolve_scope(None, Some(&home)), "global");
    unsafe { std::env::set_var("HIPPO_SCOPE", "beta") };
    assert_eq!(config::resolve_scope(None, Some(&home)), "beta");
    assert_eq!(config::resolve_scope(Some("alpha"), Some(&home)), "alpha");

    let marked = temp_dir();
    fs::create_dir_all(marked.join(".hippo")).unwrap();
    fs::write(marked.join(".hippo/scope"), "gamma\n").unwrap();
    unsafe { std::env::remove_var("HIPPO_SCOPE") };
    assert_eq!(config::resolve_scope(None, Some(&marked)), "gamma");

    let empty_config_home = temp_dir();
    unsafe {
        std::env::remove_var("HIPPO_HOME");
        std::env::set_var("HOME", &empty_config_home);
    }
    assert!(matches!(
        config::resolve_home(None),
        Err(HippoError::Config(_))
    ));

    unsafe {
        match saved_user_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match saved_home {
            Some(value) => std::env::set_var("HIPPO_HOME", value),
            None => std::env::remove_var("HIPPO_HOME"),
        }
        match saved_scope {
            Some(value) => std::env::set_var("HIPPO_SCOPE", value),
            None => std::env::remove_var("HIPPO_SCOPE"),
        }
    }
}

#[test]
fn ranking_signals() {
    let now = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    let fresh = ranking::recency_boost("2026-03-02", now);
    let one_halflife = ranking::recency_boost("2026-01-31", now);
    assert_eq!(fresh, RECENCY_WEIGHT);
    assert_eq!(one_halflife, RECENCY_WEIGHT / 2.0);
    assert_eq!(ranking::recency_boost("not-a-date", now), 0.0);
    assert_eq!(ranking::recency_boost("2026-04-01", now), RECENCY_WEIGHT);
    assert_eq!(ranking::usage_boost(0), 0.0);
    assert!(ranking::usage_boost(1) < ranking::usage_boost(10));
    assert!(ranking::usage_boost(10) < USAGE_WEIGHT);
    let base = ranking::adjust(-1.0, false, "1900-01-01", 0, now, 0.0);
    let boosted = ranking::adjust(-1.0, true, "2026-03-02", 5, now, 0.0);
    assert_eq!(base, -1.0);
    assert!(boosted < base);
}

#[test]
fn hashing_embedding() {
    let vector = embed::embed("Use full-text search for documents.");
    assert!(embed::cosine(&vector, &vector) > 0.99);
    let query = embed::embed("fulltext");
    let close = embed::embed("full-text search");
    let far = embed::embed("unrelated cooking recipes tonight");
    assert!(embed::cosine(&query, &close) > embed::cosine(&query, &far));
    let zero = embed::embed("");
    assert_eq!(zero, vec![0.0; DIM]);
    assert_eq!(embed::cosine(&zero, &zero), 0.0);
    assert_eq!(embed::cosine(&zero, &embed::embed("fulltext")), 0.0);

    let home = fresh_home(false);
    let mut store = MemoryStore::new(Some(&home), None).unwrap();
    store
        .add(
            NewMemory {
                title: "Document indexing note".into(),
                keywords: vec!["documents".into(), "retrieval".into()],
                body: "Use full-text search for documents.".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    store.reindex(false).unwrap();
    let conn = index::connect(&home).unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM memories_fts WHERE memories_fts MATCH ?1")
        .unwrap();
    let fts: Vec<String> = stmt
        .query_map([r#""fulltext""#], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(fts.is_empty());
    let hits = store.query("fulltext", 5).unwrap();
    assert_eq!(
        hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
        ["document-indexing-note"]
    );
    assert!(hits[0].snippet.contains("full-text"));

    let off_home = fresh_home(false);
    let mut off = MemoryStore::with_encoder(Some(&off_home), None, Some("off")).unwrap();
    off.add(
        NewMemory {
            title: "Document indexing note".into(),
            keywords: vec!["documents".into(), "retrieval".into()],
            body: "Use full-text search for documents.".into(),
            ..draft("", &[], "")
        },
        false,
    )
    .unwrap();
    assert!(off.query("fulltext", 5).unwrap().is_empty());
}

#[test]
fn neural_rank_keeps_a_strong_keyword_hit_ahead() {
    let home = fresh_home(false);
    let mut store = MemoryStore::new(Some(&home), None).unwrap();
    let strong = store
        .add(
            NewMemory {
                title: "Alpha beta gamma".into(),
                keywords: vec!["alpha".into(), "beta".into(), "gamma".into()],
                body: "Alpha beta gamma is the exact phrase in this note.".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    let near = store
        .add(
            NewMemory {
                title: "Other topic".into(),
                keywords: vec!["cooking".into(), "dinner".into()],
                body: "Recipes for dinner tonight with no shared query terms.".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    // FTS5 IDF is ~0 when a term hits half the rows, so BM25 stays near zero
    // until the matching terms are rare in the index.
    for i in 0..24 {
        store
            .add(
                NewMemory {
                    title: format!("Padding note {i:02}"),
                    keywords: vec![format!("pad{i}"), "unrelated".into()],
                    body: "This padding note exists so rare keywords earn a strong BM25 score."
                        .into(),
                    ..draft("", &[], "")
                },
                false,
            )
            .unwrap();
    }
    let conn = index::connect(&home).unwrap();
    index::put_vector(
        &conn,
        &strong.id,
        "bge-small",
        &strong.content_hash,
        &[1.0, 0.0],
    )
    .unwrap();
    index::put_vector(
        &conn,
        &near.id,
        "bge-small",
        &near.content_hash,
        &[0.0, 1.0],
    )
    .unwrap();
    let hits = search::query_neural(
        &conn,
        "alpha beta gamma",
        5,
        "global",
        "bge-small",
        &[0.0, 1.0],
    )
    .unwrap();
    let detail: Vec<_> = hits
        .iter()
        .map(|hit| format!("{} {:.4}", hit.id, hit.score))
        .collect();
    assert_eq!(hits[0].id, strong.id, "{detail:?}");

    let weak_home = fresh_home(false);
    let mut weak = MemoryStore::new(Some(&weak_home), None).unwrap();
    let left = weak
        .add(
            NewMemory {
                title: "Left note".into(),
                keywords: vec!["left".into()],
                body: "Nothing shared with the query words here at all, just filler text.".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    let right = weak
        .add(
            NewMemory {
                title: "Right note".into(),
                keywords: vec!["right".into()],
                body: "Also unrelated wording, stored so both notes exist.".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    let conn = index::connect(&weak_home).unwrap();
    index::put_vector(
        &conn,
        &left.id,
        "bge-small",
        &left.content_hash,
        &[1.0, 0.0],
    )
    .unwrap();
    index::put_vector(
        &conn,
        &right.id,
        "bge-small",
        &right.content_hash,
        &[0.0, 1.0],
    )
    .unwrap();
    let hits = search::query_neural(
        &conn,
        "airport origin",
        5,
        "global",
        "bge-small",
        &[0.0, 1.0],
    )
    .unwrap();
    assert_eq!(hits[0].id, right.id);
}

#[test]
fn fixture_recall_scope_and_ranking() {
    let home = fresh_home(true);
    let store = MemoryStore::new(Some(&home), None).unwrap();
    let result = store.reindex(false).unwrap();
    assert_eq!(result.added, 4);
    let hits = store.query("full text search bm25", 5).unwrap();
    assert_eq!(hits[0].id, "sqlite-fts5-basics");
    let hits = store.query("clean code readable functions", 5).unwrap();
    assert_eq!(hits[0].id, "prefer-small-functions");

    let global = MemoryStore::new(Some(&home), Some("global")).unwrap();
    let ids: Vec<_> = global
        .query("coding conventions functions style", 10)
        .unwrap()
        .into_iter()
        .map(|hit| hit.id)
        .collect();
    assert!(!ids.iter().any(|id| id == "project-alpha-conventions"));

    let alpha = MemoryStore::new(Some(&home), Some("alpha")).unwrap();
    let hits = alpha
        .query("coding conventions functions style", 10)
        .unwrap();
    assert!(hits.iter().any(|hit| hit.id == "project-alpha-conventions"));
    assert_eq!(hits[0].id, "project-alpha-conventions");
    assert_eq!(store.query("coding", 1).unwrap().len(), 1);

    let empty = fresh_home(false);
    write_ranked(&empty, "older", "2026-01-01");
    write_ranked(&empty, "newer", "2026-03-01");
    let ranked = MemoryStore::new(Some(&empty), None).unwrap();
    ranked.reindex(false).unwrap();
    let now = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    let hits = ranked.query_at("alpha beta gamma", 5, now).unwrap();
    assert_eq!(
        hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
        ["newer", "older"]
    );
    let before = ranked.query_at("alpha beta gamma", 5, now).unwrap()[0].score;
    for _ in 0..5 {
        ranked.query_at("alpha beta gamma", 5, now).unwrap();
    }
    let after = ranked.query_at("alpha beta gamma", 5, now).unwrap()[0].score;
    assert!(after < before);
}

#[test]
fn digest_and_index_lifecycle() {
    let home = fresh_home(true);
    let mut store = MemoryStore::new(Some(&home), None).unwrap();
    store.reindex(false).unwrap();
    let conn = index::connect(&home).unwrap();
    let text = render::digest(&conn, false).unwrap();
    assert!(text.starts_with("You have 4 memories"));
    assert!(text.contains("coding"));
    assert!(text.contains("hippo query"));
    let payload: serde_json::Value =
        serde_json::from_str(&render::digest(&conn, true).unwrap()).unwrap();
    assert_eq!(payload["total"], 4);
    assert!(payload["topics"][0].get("tag").is_some());
    assert!(payload["topics"][0].get("count").is_some());

    let empty = fresh_home(false);
    MemoryStore::new(Some(&empty), None)
        .unwrap()
        .reindex(false)
        .unwrap();
    let conn = index::connect(&empty).unwrap();
    assert!(render::digest(&conn, false)
        .unwrap()
        .contains("No memories yet"));

    store
        .add(
            NewMemory {
                title: "Kubernetes tips".into(),
                keywords: vec!["kubernetes".into(), "devops".into()],
                body: "Use kubectl.".into(),
                category: "ops".into(),
                ..draft("", &[], "")
            },
            false,
        )
        .unwrap();
    assert_eq!(
        store.query("kubernetes", 5).unwrap()[0].id,
        "kubernetes-tips"
    );

    store
        .update(
            "plain-language",
            MemoryPatch {
                body: Some("Totally new indexed content about brevity.".into()),
                title: None,
                keywords: None,
                tags: None,
                scope: None,
                reason: None,
                confidence: None,
            },
            false,
        )
        .unwrap();
    assert_eq!(store.reindex(true).unwrap().updated, 0);

    let path = store.path("plain-language").unwrap();
    let raw = fs::read_to_string(&path)
        .unwrap()
        .replace("plain language", "unmistakable brevity signal");
    fs::write(&path, raw).unwrap();
    assert_eq!(store.reindex(true).unwrap().updated, 1);
    assert_eq!(
        store.query("unmistakable brevity signal", 5).unwrap()[0].id,
        "plain-language"
    );

    fs::remove_file(store.path("plain-language").unwrap()).unwrap();
    assert_eq!(store.reindex(true).unwrap().deleted, 1);

    let home = fresh_home(true);
    let store = MemoryStore::new(Some(&home), None).unwrap();
    store.reindex(false).unwrap();
    let report = store.status().unwrap();
    assert!(report.is_clean());
    fs::remove_file(store.path("plain-language").unwrap()).unwrap();
    let report = store.status().unwrap();
    assert!(report.removed.iter().any(|id| id == "plain-language"));
    assert!(!report.is_clean());
    assert!(report
        .actions
        .iter()
        .any(|action| action == "hippo reindex --changed"));
}

#[test]
fn capture_gate() {
    let clean = assess(TITLE, BODY, &["prefs".into(), "durable".into()]);
    assert_eq!(clean.suggested, "high");
    assert!(clean.warnings.is_empty());

    let short = assess("Short", "tiny", &["one".into()]);
    assert_eq!(short.suggested, "medium");
    assert_eq!(
        short.warnings,
        [
            "body is shorter than 40 characters",
            "title is shorter than 8 characters",
            "fewer than 2 keywords",
        ]
    );
    assert_eq!(
        assess("12345678", &"x".repeat(40), &["a".into(), "b".into()]).suggested,
        "high"
    );
    assert_eq!(
        assess("1234567", &"x".repeat(40), &["a".into(), "b".into()]).suggested,
        "medium"
    );
    assert_eq!(
        assess("12345678", &"x".repeat(39), &["a".into(), "b".into()]).suggested,
        "medium"
    );
    assert_eq!(
        assess("12345678", &"x".repeat(40), &["a".into()]).suggested,
        "medium"
    );

    for text in [
        "password=hunter2",
        "api_key=abcd",
        "secret=xyz",
        &format!("sk-{}", "a".repeat(16)),
        "AKIAIOSFODNN7EXAMPLE",
        "123456789012",
    ] {
        let result = assess(
            TITLE,
            &format!("{BODY} {text}"),
            &["prefs".into(), "durable".into()],
        );
        assert_eq!(result.suggested, "low", "{text}");
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning == "text looks like a secret"));
    }
    assert_eq!(
        assess(
            "password=hunter2",
            BODY,
            &["prefs".into(), "durable".into()]
        )
        .suggested,
        "low"
    );
    assert_eq!(
        assess(
            TITLE,
            &format!("{BODY} sk-{}", "a".repeat(15)),
            &["prefs".into(), "durable".into()]
        )
        .suggested,
        "high"
    );
    assert_eq!(
        assess(
            TITLE,
            &format!("{BODY} 12345678901"),
            &["prefs".into(), "durable".into()]
        )
        .suggested,
        "high"
    );
    assert_eq!(
        assess(TITLE, BODY, &["password=hunter2".into(), "other".into()]).suggested,
        "low"
    );
    assert_eq!(
        assess(TITLE, BODY, &["password".into(), "other".into()]).suggested,
        "high"
    );

    let mixed = assess(
        "Short",
        &format!("{BODY} password=hunter2"),
        &["one".into()],
    );
    assert_eq!(mixed.suggested, "low");
    assert_eq!(
        mixed.warnings,
        [
            "title is shorter than 8 characters",
            "fewer than 2 keywords",
            "text looks like a secret",
        ]
    );

    let low = assess(
        TITLE,
        "password=hunter2",
        &["prefs".into(), "durable".into()],
    );
    let medium = assess("Short", BODY, &["prefs".into(), "durable".into()]);
    let high = assess(TITLE, BODY, &["prefs".into(), "durable".into()]);
    capture::gate(None, &high, false).unwrap();
    capture::gate(None, &medium, false).unwrap();
    capture::gate(Some("high"), &medium, false).unwrap();
    capture::gate(Some("medium"), &high, false).unwrap();
    assert!(capture::gate(None, &low, false).is_err());
    assert!(capture::gate(Some("low"), &high, false).is_err());
    assert!(capture::gate(Some("high"), &low, false).is_err());
    capture::gate(Some("low"), &low, true).unwrap();

    let home = fresh_home(false);
    let memory = store::write_new(
        &home,
        NewMemory {
            reason: Some("durable preference".into()),
            confidence: Some("high".into()),
            ..draft(TITLE, &["prefs", "durable"], BODY)
        },
    )
    .unwrap();
    let updated = store::update(
        &home,
        &memory.id,
        MemoryPatch {
            body: Some("This preference stays true across sessions and projects, still.".into()),
            title: None,
            keywords: None,
            tags: None,
            scope: None,
            reason: None,
            confidence: None,
        },
    )
    .unwrap();
    assert_eq!(updated.reason.as_deref(), Some("durable preference"));
    assert_eq!(updated.confidence.as_deref(), Some("high"));
    let raw = fs::read_to_string(&updated.path).unwrap();
    assert!(raw.contains("reason: durable preference"));
    assert!(raw.contains("confidence: high"));

    assert!(store::write_new(
        &home,
        NewMemory {
            confidence: Some("sometimes".into()),
            ..draft(TITLE, &["prefs", "durable"], BODY)
        },
    )
    .is_err());
}

#[test]
fn hygiene_and_consolidate() {
    let uv = mem(
        "prefer-uv",
        "Prefer uv over pip",
        &["uv", "pip", "tooling"],
        "2026-01-01",
        "Use uv for installs. Do not use pip.",
    );
    let pip = mem(
        "prefer-pip",
        "Prefer pip over uv",
        &["uv", "pip", "tooling"],
        "2026-01-01",
        "Use pip for installs. Do not use uv.",
    );
    let finding = hygiene::compare_pair(&uv, &pip).unwrap();
    assert_eq!(finding.kind, "contradiction");
    assert!(finding.tokens.iter().any(|token| token == "uv"));
    assert!(finding.tokens.iter().any(|token| token == "pip"));

    let original = mem(
        "prefer-uv",
        "Prefer uv over pip",
        &["uv", "pip", "tooling"],
        "2026-01-01",
        "Use uv for environment and install workflows. Do not use pip.",
    );
    let copy = mem(
        "uv-not-pip",
        "Prefer uv not pip",
        &["uv", "pip", "python"],
        "2026-02-01",
        "Use uv for environment and install workflows. Do not use pip.",
    );
    assert_eq!(
        hygiene::compare_pair(&original, &copy).unwrap().kind,
        "duplicate"
    );
    let mut hits = std::collections::HashMap::new();
    hits.insert("prefer-uv".into(), 4);
    hits.insert("uv-not-pip".into(), 0);
    let report = analyze(&[original, copy], &hits);
    assert!(report.contradictions.is_empty());
    assert_eq!(report.duplicates[0].keeper, "prefer-uv");
    assert_eq!(report.duplicates[0].members, ["prefer-uv", "uv-not-pip"]);

    assert!(hygiene::compare_pair(
        &mem(
            "a",
            "Write in plain language",
            &["writing", "clarity"],
            "2026-01-01",
            "Use short sentences."
        ),
        &mem(
            "b",
            "SQLite FTS5 basics",
            &["sqlite", "fts5"],
            "2026-01-01",
            "Use bm25 to rank matches."
        ),
    )
    .is_none());

    let home = fresh_home(true);
    let report = analyze(
        &store::load_all(&home).unwrap(),
        &std::collections::HashMap::new(),
    );
    assert!(report.duplicates.is_empty());
    assert!(report.contradictions.is_empty());

    let empty = fresh_home(false);
    let mut store = MemoryStore::new(Some(&empty), None).unwrap();
    for (title, keywords, body) in [
        (
            "Prefer uv over pip",
            vec!["uv", "pip", "tooling"],
            "Use uv for environment and install workflows. Do not use pip.",
        ),
        (
            "Prefer uv not pip",
            vec!["uv", "pip", "python"],
            "Use uv for environment and install workflows. Do not use pip. Extra note about lockfiles.",
        ),
        (
            "Prefer pip over uv",
            vec!["uv", "pip", "tooling"],
            "Use pip for installs. Do not use uv.",
        ),
    ] {
        store
            .add(
                NewMemory {
                    title: title.into(),
                    keywords: keywords.into_iter().map(str::to_string).collect(),
                    body: body.into(),
                    category: "preferences".into(),
                    ..draft("", &[], "")
                },
                false,
            )
            .unwrap();
    }
    let preview = store.consolidate(false).unwrap();
    assert_eq!(preview.duplicates.len(), 1);
    let members: std::collections::HashSet<_> =
        preview.duplicates[0].members.iter().cloned().collect();
    assert_eq!(
        members,
        ["prefer-uv-over-pip", "prefer-uv-not-pip"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
    assert_eq!(preview.contradictions.len(), 2);
    assert!(!preview.applied);
    let keeper = preview.duplicates[0].keeper.clone();
    let removed_expected: Vec<String> = preview.duplicates[0]
        .members
        .iter()
        .filter(|id| *id != &keeper)
        .cloned()
        .collect();
    let applied = store.consolidate(true).unwrap();
    assert_eq!(applied.removed, removed_expected);
    let remaining: std::collections::HashSet<_> = store::iter_memory_files(&empty)
        .iter()
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        remaining,
        [keeper.as_str(), "prefer-pip-over-uv"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
    let kept = store.show(&keeper).unwrap();
    assert!(kept.body.contains("lockfiles"));
    assert!(kept.body.contains("environment and install workflows"));
    assert!(kept
        .keywords
        .iter()
        .any(|item| item == "tooling" || item == "python"));
    assert!(store.consolidate(false).unwrap().duplicates.is_empty());

    let empty = fresh_home(false);
    let mut store = MemoryStore::new(Some(&empty), None).unwrap();
    store
        .add(
            draft(
                "Prefer uv over pip",
                &["uv", "pip", "tooling"],
                "Use uv for environment and install workflows. Do not use pip.",
            ),
            false,
        )
        .unwrap();
    store
        .add(
            draft(
                "Prefer uv not pip",
                &["uv", "pip", "python"],
                "Use uv for environment and install workflows. Do not use pip.",
            ),
            false,
        )
        .unwrap();
    assert!(store
        .status()
        .unwrap()
        .actions
        .iter()
        .any(|action| action == "hippo consolidate"));
}

#[test]
fn cli_contract() {
    let home = fresh_home(true);
    assert_eq!(code(&hippo(&home, &["reindex", "--all"], None)), 0);
    let result = hippo(&home, &["query", "bm25 search", "--json"], None);
    assert_eq!(code(&result), 0);
    let hits: Vec<serde_json::Value> = serde_json::from_str(&stdout(&result)).unwrap();
    assert_eq!(hits[0]["id"], "sqlite-fts5-basics");
    for key in ["id", "path", "title", "score", "snippet"] {
        assert!(hits[0].get(key).is_some(), "{key}");
    }

    let digest = hippo(&home, &["digest", "--format", "json"], None);
    assert_eq!(code(&digest), 0, "{}", stderr(&digest));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&digest)).unwrap();
    assert_eq!(payload["total"], 4);
    assert!(payload["topics"][0].get("tag").is_some());

    let manifest = hippo(&home, &["manifest", "--format", "json"], None);
    assert_eq!(code(&manifest), 0);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout(&manifest)).unwrap();
    assert!(entries.iter().any(|entry| entry["id"] == "plain-language"));

    let added = hippo(
        &home,
        &[
            "add",
            "--title",
            "Docker cheatsheet",
            "--keywords",
            "docker,containers",
            "--category",
            "ops",
            "--json",
        ],
        Some("docker run -it ubuntu bash\n"),
    );
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&added)).unwrap();
    assert_eq!(payload["id"], "docker-cheatsheet");

    let added = hippo(
        &home,
        &[
            "add",
            "--title",
            "Prefers uv over pip",
            "--keywords",
            "uv,tooling",
            "--category",
            "preferences",
            "--source",
            "cursor",
            "--json",
        ],
        Some("Use uv for env + install workflows.\n"),
    );
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&added)).unwrap();
    let shown = hippo(&home, &["show", payload["id"].as_str().unwrap()], None);
    assert!(stdout(&shown).contains("source: cursor"));

    let missing = hippo(&home, &["path", "does-not-exist"], None);
    assert_eq!(code(&missing), 2);

    let status = hippo(&home, &["status", "--json"], None);
    assert_eq!(code(&status), 0, "{}", stderr(&status));
    let report: serde_json::Value = serde_json::from_str(&stdout(&status)).unwrap();
    for key in ["indexed", "new", "changed", "removed", "git", "actions"] {
        assert!(report.get(key).is_some(), "{key}");
    }

    let before = files(&home);
    let saved = hippo(
        &home,
        &[
            "add",
            "--title",
            TITLE,
            "--keywords",
            "prefs,durable",
            "--confidence",
            "high",
            "--reason",
            "durable preference",
            "--json",
        ],
        Some(&format!("{BODY}\n")),
    );
    assert_eq!(code(&saved), 0, "{}", stderr(&saved));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&saved)).unwrap();
    assert!(payload.get("warnings").is_none());
    let raw = stdout(&hippo(
        &home,
        &["show", payload["id"].as_str().unwrap()],
        None,
    ));
    assert!(raw.contains("confidence: high"));
    assert!(raw.contains("reason: durable preference"));
    assert_eq!(files(&home).len(), before.len() + 1);

    let before = files(&home);
    let rejected = hippo(
        &home,
        &[
            "add",
            "--title",
            TITLE,
            "--keywords",
            "prefs,durable",
            "--confidence",
            "low",
            "--json",
        ],
        Some(&format!("{BODY}\n")),
    );
    assert_eq!(code(&rejected), 2);
    assert_eq!(files(&home), before);

    let forced = hippo(
        &home,
        &[
            "add",
            "--title",
            "Forced low confidence note",
            "--keywords",
            "prefs,durable",
            "--confidence",
            "low",
            "--force",
            "--json",
        ],
        Some(&format!("{BODY}\n")),
    );
    assert_eq!(code(&forced), 0, "{}", stderr(&forced));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&forced)).unwrap();
    assert!(stdout(&hippo(
        &home,
        &["show", payload["id"].as_str().unwrap()],
        None
    ))
    .contains("confidence: low"));

    let before = files(&home);
    let secret = hippo(
        &home,
        &[
            "add",
            "--title",
            TITLE,
            "--keywords",
            "prefs,durable",
            "--confidence",
            "high",
            "--json",
        ],
        Some("The saved login contains password=hunter2 and must not be copied.\n"),
    );
    assert_eq!(code(&secret), 2);
    assert_eq!(files(&home), before);

    let empty = fresh_home(false);
    let secret = serde_json::json!({
        "title": TITLE,
        "keywords": ["prefs", "durable"],
        "body": "The saved login contains password=hunter2 and must not be copied.",
        "confidence": "high",
    });
    let rejected = hippo(&empty, &["add", "--from-json"], Some(&secret.to_string()));
    assert_eq!(code(&rejected), 2);
    assert!(files(&empty).is_empty());
    let clean = serde_json::json!({
        "title": TITLE,
        "keywords": ["prefs", "durable"],
        "body": BODY,
        "reason": "from json",
        "confidence": "high",
    });
    let saved = hippo(
        &empty,
        &["add", "--from-json", "--json"],
        Some(&clean.to_string()),
    );
    assert_eq!(code(&saved), 0, "{}", stderr(&saved));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&saved)).unwrap();
    assert!(stdout(&hippo(
        &empty,
        &["show", payload["id"].as_str().unwrap()],
        None
    ))
    .contains("reason: from json"));

    let before = files(&home);
    let invalid = hippo(
        &home,
        &[
            "add",
            "--title",
            TITLE,
            "--keywords",
            "prefs,durable",
            "--confidence",
            "sometimes",
            "--json",
        ],
        Some(&format!("{BODY}\n")),
    );
    assert_eq!(code(&invalid), 2);
    assert_eq!(files(&home), before);

    let short = hippo(
        &home,
        &[
            "add",
            "--title",
            "Short body note",
            "--keywords",
            "prefs,durable",
            "--json",
        ],
        Some("short\n"),
    );
    assert_eq!(code(&short), 0, "{}", stderr(&short));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&short)).unwrap();
    let warnings = payload["warnings"].as_array().unwrap();
    assert!(warnings
        .iter()
        .any(|warning| warning == "body is shorter than 40 characters"));
    let text = hippo(
        &home,
        &[
            "add",
            "--title",
            "Another durable note",
            "--keywords",
            "prefs,durable",
        ],
        Some("short\n"),
    );
    assert_eq!(code(&text), 0);
    assert!(stderr(&text).contains("warning: body is shorter than 40 characters"));

    let created = hippo(
        &home,
        &[
            "add",
            "--title",
            "Update target note",
            "--keywords",
            "prefs,durable",
            "--confidence",
            "high",
            "--reason",
            "durable preference",
            "--json",
        ],
        Some(&format!("{BODY}\n")),
    );
    let memory_id = serde_json::from_str::<serde_json::Value>(&stdout(&created)).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = stdout(&hippo(&home, &["show", &memory_id], None));
    let rejected = hippo(&home, &["update", &memory_id, "--confidence", "low"], None);
    assert_eq!(code(&rejected), 2);
    assert_eq!(stdout(&hippo(&home, &["show", &memory_id], None)), before);
    let secret = hippo(
        &home,
        &["update", &memory_id, "--body", "password=hunter2"],
        None,
    );
    assert_eq!(code(&secret), 2);
    assert_eq!(stdout(&hippo(&home, &["show", &memory_id], None)), before);
    let retitled = hippo(
        &home,
        &["update", &memory_id, "--title", "Durable preference notes"],
        None,
    );
    assert_eq!(code(&retitled), 0, "{}", stderr(&retitled));
    let forced = hippo(
        &home,
        &[
            "update",
            &memory_id,
            "--confidence",
            "low",
            "--force",
            "--json",
        ],
        None,
    );
    assert_eq!(code(&forced), 0, "{}", stderr(&forced));
    let raw = stdout(&hippo(&home, &["show", &memory_id], None));
    assert!(raw.contains("confidence: low"));
    assert!(raw.contains("reason: durable preference"));

    let empty = fresh_home(false);
    hippo(
        &empty,
        &[
            "add",
            "--title",
            "Prefer uv over pip",
            "--keywords",
            "uv,pip,tooling",
            "--json",
        ],
        Some("Use uv for environment and install workflows. Do not use pip.\n"),
    );
    let noticed = hippo(
        &empty,
        &[
            "add",
            "--title",
            "Prefer uv not pip",
            "--keywords",
            "uv,pip,python",
            "--json",
        ],
        Some("Use uv for environment and install workflows. Do not use pip.\n"),
    );
    assert_eq!(code(&noticed), 0, "{}", stderr(&noticed));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&noticed)).unwrap();
    assert_eq!(payload["notices"][0]["kind"], "duplicate");
    assert_eq!(payload["notices"][0]["other_id"], "prefer-uv-over-pip");

    let empty = fresh_home(false);
    hippo(
        &empty,
        &[
            "add",
            "--title",
            "Prefer pip over uv",
            "--keywords",
            "uv,pip,tooling",
        ],
        Some("Use pip for installs. Do not use uv.\n"),
    );
    hippo(
        &empty,
        &[
            "add",
            "--title",
            "Prefer uv over pip",
            "--keywords",
            "uv,pip,tooling",
        ],
        Some("Use uv for installs. Do not use pip.\n"),
    );
    let report = hippo(&empty, &["consolidate", "--json"], None);
    assert_eq!(code(&report), 0, "{}", stderr(&report));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&report)).unwrap();
    assert_eq!(payload["applied"], false);
    assert_eq!(payload["duplicates"].as_array().unwrap().len(), 0);
    assert!(!payload["contradictions"][0]["tokens"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn pi_adapter_uses_the_cli() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters/pi/bin/hippo-pi.sh");
    let bin_dir = PathBuf::from(env!("CARGO_BIN_EXE_hippo"))
        .parent()
        .unwrap()
        .to_path_buf();
    let path_var = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let run = |home: &Path, args: &[&str], stdin: Option<&str>| {
        let mut cmd = Command::new("bash");
        cmd.arg(&script).arg("--home").arg(home).args(args);
        cmd.env("PATH", &path_var);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
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
    };

    let home = fresh_home(true);
    MemoryStore::new(Some(&home), None)
        .unwrap()
        .reindex(false)
        .unwrap();
    let digest = run(&home, &["digest"], None);
    assert_eq!(code(&digest), 0, "{}", stderr(&digest));
    assert!(stdout(&digest).contains("You have 4 memories"));
    let recall = run(&home, &["recall", "bm25"], None);
    assert_eq!(code(&recall), 0, "{}", stderr(&recall));
    let hits: Vec<serde_json::Value> = serde_json::from_str(&stdout(&recall)).unwrap();
    assert!(hits.iter().any(|hit| hit["id"] == "sqlite-fts5-basics"));

    let empty = fresh_home(false);
    let payload = serde_json::json!({
        "title": "Pi adapter note",
        "keywords": ["pi", "adapter", "stub"],
        "body": "The pi harness saves memories only through the hippo CLI contract.",
        "reason": "portability proof",
        "confidence": "high",
        "category": "coding",
    });
    let saved = run(&empty, &["save"], Some(&payload.to_string()));
    assert_eq!(code(&saved), 0, "{}", stderr(&saved));
    let found = run(&empty, &["recall", "pi adapter"], None);
    assert_eq!(code(&found), 0, "{}", stderr(&found));
    let hits: Vec<serde_json::Value> = serde_json::from_str(&stdout(&found)).unwrap();
    assert!(hits.iter().any(|hit| hit["id"] == "pi-adapter-note"));

    let secret = serde_json::json!({
        "title": "Login note",
        "keywords": ["login", "secret"],
        "body": "The saved login contains password=hunter2 and must not be copied.",
        "confidence": "high",
    });
    let before = files(&empty);
    let rejected = run(&empty, &["save"], Some(&secret.to_string()));
    assert_eq!(code(&rejected), 2);
    assert!(stderr(&rejected).contains("Refusing to save"));
    assert_eq!(files(&empty), before);

    let nul = serde_json::json!({
        "title": "Login note\u{0000}password=hunter2",
        "keywords": ["login", "note"],
        "body": "A durable note about where credentials live, with no secret in it.",
        "confidence": "low",
    });
    let before = files(&empty);
    let rejected = run(&empty, &["save"], Some(&nul.to_string()));
    assert_eq!(code(&rejected), 2);
    assert!(stderr(&rejected).contains("NUL"));
    assert_eq!(files(&empty), before);
}
