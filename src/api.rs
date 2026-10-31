use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastembed::TextEmbedding;
use serde::Serialize;

use crate::capture::{self, Assessment};
use crate::config::{self, GLOBAL_SCOPE};
use crate::encoder::{self, Encoder};
use crate::error::{HippoError, Result};
use crate::hygiene::{self, Cluster, HygieneReport, Notice};
use crate::index::{self, ReindexResult};
use crate::ranking;
use crate::render;
use crate::search::{self, Hit, Recall};
use crate::store::{self, Memory, MemoryPatch, NewMemory};
use crate::vcs::{self, GitStatus, SyncReport};

const WRITE_LOCK: &str = "write.lock";

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub indexed: usize,
    pub new: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub git: GitStatus,
    pub actions: Vec<String>,
}

impl StatusReport {
    pub fn is_clean(&self) -> bool {
        self.new.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

pub struct MemoryStore {
    pub home: PathBuf,
    pub scope: String,
    pub capture_warnings: Vec<String>,
    encoder: Encoder,
    model: Mutex<Option<TextEmbedding>>,
}

impl MemoryStore {
    pub fn new(home: Option<&Path>, scope: Option<&str>) -> Result<Self> {
        Self::with_encoder(home, scope, None)
    }

    pub fn with_encoder(
        home: Option<&Path>,
        scope: Option<&str>,
        embed: Option<&str>,
    ) -> Result<Self> {
        let name = config::resolve_embed(embed).unwrap_or_else(|| "hash".to_string());
        Ok(Self {
            home: config::resolve_home(home)?,
            scope: config::resolve_scope(scope, None),
            capture_warnings: Vec::new(),
            encoder: Encoder::parse(&name)?,
            model: Mutex::new(None),
        })
    }

    pub fn encoder_name(&self) -> &'static str {
        self.encoder.name
    }

    pub fn query(&self, text: &str, k: usize) -> Result<Vec<Hit>> {
        self.query_at(text, k, ranking::today())
    }

    pub fn query_at(&self, text: &str, k: usize, now: chrono::NaiveDate) -> Result<Vec<Hit>> {
        let conn = index::connect(&self.home)?;
        let hits = if self.encoder.is_neural() {
            let vector = self.embed_one(text)?;
            search::query_neural(&conn, text, k, &self.scope, self.encoder.name, &vector)?
        } else {
            let recall = if self.encoder.name == "off" {
                Recall::Off
            } else {
                Recall::Hash
            };
            search::query(&conn, text, k, &self.scope, now, recall)?
        };
        let ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
        index::record_access(&conn, &ids, &now.format("%Y-%m-%d").to_string())?;
        Ok(hits)
    }

    pub fn manifest(&self, as_json: bool) -> Result<String> {
        let conn = index::connect(&self.home)?;
        render::manifest(&conn, as_json)
    }

    pub fn digest(&self, as_json: bool) -> Result<String> {
        let conn = index::connect(&self.home)?;
        render::digest(&conn, as_json)
    }

    pub fn add(&mut self, mut draft: NewMemory, force: bool) -> Result<Memory> {
        self.capture_warnings.clear();
        if draft.scope.is_empty() {
            draft.scope = GLOBAL_SCOPE.to_string();
        }
        if draft.category.is_empty() {
            draft.category = store::DEFAULT_CATEGORY.to_string();
        }
        self.guard(
            &draft.title,
            &draft.body,
            &draft.keywords,
            draft.confidence.as_deref(),
            force,
        )?;
        let memory = store::write_new(&self.home, draft)?;
        let conn = index::connect(&self.home)?;
        self.index_memory(&conn, &memory)?;
        Ok(memory)
    }

    pub fn update(&mut self, memory_id: &str, patch: MemoryPatch, force: bool) -> Result<Memory> {
        self.capture_warnings.clear();
        let current = store::read_memory(&store::find_path(&self.home, memory_id)?)?;
        let title = patch.title.clone().unwrap_or(current.title);
        let body = patch.body.clone().unwrap_or(current.body);
        let keywords = patch.keywords.clone().unwrap_or(current.keywords);
        let confidence = patch.confidence.as_deref();
        self.guard(&title, &body, &keywords, confidence, force)?;
        let memory = store::update(&self.home, memory_id, patch)?;
        let conn = index::connect(&self.home)?;
        self.index_memory(&conn, &memory)?;
        Ok(memory)
    }

    pub fn review(&self, memory: &Memory) -> Result<Vec<Notice>> {
        let others = store::load_all(&self.home)?;
        Ok(hygiene::notices_for(memory, &others))
    }

    pub fn consolidate(&self, apply: bool) -> Result<HygieneReport> {
        let memories = store::load_all(&self.home)?;
        let conn = index::connect(&self.home)?;
        let hits = index::hit_counts(&conn)?;
        let mut report = hygiene::analyze(&memories, &hits);
        if !apply {
            return Ok(report);
        }
        let by_id: HashMap<String, Memory> = memories
            .iter()
            .cloned()
            .map(|memory| (memory.id.clone(), memory))
            .collect();
        let mut removed = Vec::new();
        for cluster in &report.duplicates {
            removed.extend(self.merge_cluster(cluster, &by_id)?);
        }
        report.applied = true;
        report.removed = removed;
        Ok(report)
    }

    pub fn show(&self, memory_id: &str) -> Result<Memory> {
        store::read_memory(&store::find_path(&self.home, memory_id)?)
    }

    pub fn path(&self, memory_id: &str) -> Result<PathBuf> {
        store::find_path(&self.home, memory_id)
    }

    /// Held for a whole write so parallel agents queue instead of colliding on git.
    pub fn lock_writes(&self) -> Result<File> {
        let path = index::db_path(&self.home).with_file_name(WRITE_LOCK);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(HippoError::unexpected)?;
        }
        let file = File::create(path).map_err(HippoError::unexpected)?;
        file.lock().map_err(HippoError::unexpected)?;
        Ok(file)
    }

    /// Reindex hand edits, then commit and push the memory folder.
    pub fn sync(&self) -> Result<SyncReport> {
        self.reindex(true)?;
        vcs::sync(&self.home, &store::memory_dir(&self.home))
    }

    pub fn is_memory_file(&self, path: &Path) -> bool {
        let memory_dir = store::memory_dir(&self.home);
        let resolve = |path: &Path| {
            path.canonicalize()
                .or_else(|_| std::path::absolute(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        path.extension().is_some_and(|ext| ext == "md")
            && resolve(path).starts_with(resolve(&memory_dir))
    }

    pub fn reindex(&self, changed_only: bool) -> Result<ReindexResult> {
        let memories = store::load_all(&self.home)?;
        let conn = index::connect(&self.home)?;
        let result = if changed_only {
            reindex_changed(self, &conn, &memories)?
        } else {
            reindex_all(self, &conn, &memories)?
        };
        Ok(result)
    }

    pub fn status(&self) -> Result<StatusReport> {
        let memories = store::load_all(&self.home)?;
        let fs_hashes: HashMap<String, String> = memories
            .iter()
            .map(|memory| (memory.id.clone(), memory.content_hash.clone()))
            .collect();
        let conn = index::connect(&self.home)?;
        let stored = index::stored_hashes(&conn)?;
        let mut new: Vec<String> = fs_hashes
            .keys()
            .filter(|id| !stored.contains_key(*id))
            .cloned()
            .collect();
        let mut removed: Vec<String> = stored
            .keys()
            .filter(|id| !fs_hashes.contains_key(*id))
            .cloned()
            .collect();
        let mut changed: Vec<String> = fs_hashes
            .iter()
            .filter(|(id, hash)| {
                stored
                    .get(*id)
                    .is_some_and(|stored_hash| stored_hash != *hash)
            })
            .map(|(id, _)| id.clone())
            .collect();
        new.sort();
        removed.sort();
        changed.sort();
        let git = vcs::status(&self.home);
        let mut report = StatusReport {
            indexed: stored.len(),
            new,
            changed,
            removed,
            git: git.clone(),
            actions: Vec::new(),
        };
        report.actions = suggest_actions(&report, &git, &memories);
        if self.encoder.is_neural() {
            let (ready, total) = index::vector_coverage(&conn, self.encoder.name)?;
            if ready < total {
                report.actions.insert(
                    0,
                    format!(
                        "hippo reindex --all  # {ready} of {total} notes have {} vectors",
                        self.encoder.name
                    ),
                );
            }
        }
        Ok(report)
    }

    pub fn bench_encoder(&self) -> Result<encoder::Bench> {
        self.encoder.bench()
    }

    fn index_memory(&self, conn: &rusqlite::Connection, memory: &Memory) -> Result<()> {
        index::upsert(conn, memory)?;
        if self.encoder.is_neural() {
            let vector = self.embed_one(&index::indexed_text(memory))?;
            index::put_vector(
                conn,
                &memory.id,
                self.encoder.name,
                &memory.content_hash,
                &vector,
            )?;
        }
        Ok(())
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f64>> {
        let mut slot = self.model.lock().map_err(HippoError::unexpected)?;
        if slot.is_none() {
            *slot = Some(self.encoder.load_model()?);
        }
        let model = slot.as_mut().expect("model loaded");
        let embedded = self.encoder.embed(model, &[text])?;
        Ok(embedded
            .into_iter()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|value| f64::from(value))
            .collect())
    }

    fn guard(
        &mut self,
        title: &str,
        body: &str,
        keywords: &[String],
        confidence: Option<&str>,
        force: bool,
    ) -> Result<()> {
        capture::check_confidence(confidence)?;
        let assessment: Assessment = capture::assess(title, body, keywords);
        capture::gate(confidence, &assessment, force)?;
        self.capture_warnings = assessment.warnings;
        Ok(())
    }

    fn merge_cluster(
        &self,
        cluster: &Cluster,
        by_id: &HashMap<String, Memory>,
    ) -> Result<Vec<String>> {
        let keeper = &by_id[&cluster.keeper];
        let others: Vec<&Memory> = cluster
            .members
            .iter()
            .filter(|id| *id != &cluster.keeper)
            .map(|id| &by_id[id])
            .collect();
        let mut body = keeper.body.trim_end().to_string();
        for other in &others {
            let extra = other.body.trim();
            if !extra.is_empty() && !body.contains(extra) {
                body.push_str(&format!("\n\nMerged from `{}`:\n\n{extra}", other.id));
            }
        }
        let keywords = union_lists(
            std::iter::once(keeper.keywords.clone())
                .chain(others.iter().map(|other| other.keywords.clone())),
        );
        let tags = union_lists(
            std::iter::once(keeper.tags.clone())
                .chain(others.iter().map(|other| other.tags.clone())),
        );
        let memory = store::update(
            &self.home,
            &keeper.id,
            MemoryPatch {
                title: None,
                keywords: Some(keywords),
                tags: Some(tags),
                body: Some(body),
                scope: None,
                reason: None,
                confidence: None,
            },
        )?;
        let conn = index::connect(&self.home)?;
        self.index_memory(&conn, &memory)?;
        let mut removed = Vec::new();
        for other in others {
            store::delete(&self.home, &other.id)?;
            index::delete(&conn, &other.id)?;
            removed.push(other.id.clone());
        }
        Ok(removed)
    }
}

fn reindex_all(
    store: &MemoryStore,
    conn: &rusqlite::Connection,
    memories: &[Memory],
) -> Result<ReindexResult> {
    let stored = index::stored_hashes(conn)?;
    for memory in memories {
        store.index_memory(conn, memory)?;
    }
    let live: std::collections::HashSet<&str> =
        memories.iter().map(|memory| memory.id.as_str()).collect();
    for stale_id in stored.keys() {
        if !live.contains(stale_id.as_str()) {
            index::delete(conn, stale_id)?;
        }
    }
    Ok(ReindexResult {
        added: memories.len() as i64,
        ..ReindexResult::default()
    })
}

fn reindex_changed(
    store: &MemoryStore,
    conn: &rusqlite::Connection,
    memories: &[Memory],
) -> Result<ReindexResult> {
    let stored = index::stored_hashes(conn)?;
    let mut result = ReindexResult::default();
    let mut seen = std::collections::HashSet::new();
    for memory in memories {
        seen.insert(memory.id.clone());
        let stale_vector = store.encoder.is_neural()
            && !index::vector_is_current(
                conn,
                &memory.id,
                store.encoder.name,
                &memory.content_hash,
            )?;
        match stored.get(&memory.id) {
            None => {
                store.index_memory(conn, memory)?;
                result.added += 1;
            }
            Some(hash) if hash != &memory.content_hash || stale_vector => {
                store.index_memory(conn, memory)?;
                result.updated += 1;
            }
            _ => {}
        }
    }
    for stale_id in stored.keys() {
        if !seen.contains(stale_id) {
            index::delete(conn, stale_id)?;
            result.deleted += 1;
        }
    }
    Ok(result)
}

fn suggest_actions(report: &StatusReport, git: &GitStatus, memories: &[Memory]) -> Vec<String> {
    let mut actions = Vec::new();
    if !report.is_clean() {
        actions.push("hippo reindex --changed".into());
    }
    let findings = hygiene::analyze(memories, &HashMap::new());
    if !findings.duplicates.is_empty() {
        actions.push("hippo consolidate".into());
    }
    if !findings.contradictions.is_empty() {
        actions.push("resolve memory contradictions (hippo consolidate)".into());
    }
    if git.is_repo && git.dirty {
        actions.push("commit memory changes in the data repo".into());
    }
    if git.is_repo && git.ahead > 0 {
        actions.push(format!("push {} unpushed commit(s)", git.ahead));
    }
    actions
}

fn union_lists(groups: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for group in groups {
        for item in group {
            let key = item.trim().to_lowercase();
            if !key.is_empty() && seen.insert(key) {
                merged.push(item);
            }
        }
    }
    merged
}
