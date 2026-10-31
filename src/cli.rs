use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;

use crate::adapter;
use crate::api::MemoryStore;
use crate::config::{self, GLOBAL_SCOPE};
use crate::encoder::Encoder;
use crate::error::{HippoError, Result};
use crate::hygiene::{Finding, HygieneReport, Notice};
use crate::store::{MemoryPatch, NewMemory, DEFAULT_CATEGORY};

#[derive(Parser)]
#[command(name = "hippo", version, about = "Hippo personal memory.")]
struct Cli {
    /// Data repo path (overrides env/config).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Active scope (default: resolved / global).
    #[arg(long, global = true)]
    scope: Option<String>,
    /// Vector encoder: hash, off, bge-small, bge-base, or bge-m3.
    #[arg(long, global = true)]
    embed: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum EncoderCommand {
    /// List the encoders Hippo can run.
    List,
    /// Time model load and one query embedding. Process startup is not included.
    Bench,
}

#[derive(Subcommand)]
enum Command {
    /// Smart recall: ranked (id, path, title, score, snippet).
    Query {
        text: String,
        #[arg(long, default_value_t = 5)]
        k: usize,
        #[arg(long)]
        json: bool,
    },
    /// Create a memory and index it.
    Add {
        #[arg(long = "from-json")]
        from_json: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        keywords: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, default_value = DEFAULT_CATEGORY)]
        category: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        confidence: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Edit an existing memory and reindex it.
    Update {
        memory_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        keywords: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        confidence: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Rebuild the index from markdown (defaults to --all).
    Reindex {
        #[arg(long = "all")]
        all: bool,
        #[arg(long)]
        changed: bool,
    },
    /// Full catalog for tooling and debugging.
    Manifest {
        #[arg(long, default_value = "md")]
        format: String,
    },
    /// Constant-size session primer: count and top topics.
    Digest {
        #[arg(long, default_value = "md")]
        format: String,
    },
    /// Report near-duplicates and contradictions.
    Consolidate {
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Reindex, then commit and push memory changes in the data repo.
    Sync {
        /// Only sync when this file is inside the memory folder.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// List encoders, or time the active one.
    Encoder {
        #[command(subcommand)]
        command: EncoderCommand,
    },
    /// Report index drift, git state, and suggested actions.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Print a memory's raw content.
    Show { memory_id: String },
    /// Print a memory's file path.
    Path { memory_id: String },
    /// Install an adapter shipped inside this binary.
    Adapter {
        #[command(subcommand)]
        command: AdapterCommand,
    },
}

#[derive(Subcommand)]
enum AdapterCommand {
    /// List adapters shipped with this binary.
    List,
    /// Write an adapter into the agent config and link it.
    Install { name: String },
}

pub fn run() -> Result<()> {
    let Cli {
        home,
        scope,
        embed,
        command,
    } = Cli::parse();
    let command = match command {
        Command::Adapter { command } => return run_adapter(command),
        Command::Encoder { command } => return run_encoder(command, embed.as_deref()),
        other => other,
    };
    let mut store = MemoryStore::with_encoder(home.as_deref(), scope.as_deref(), embed.as_deref())?;
    match command {
        Command::Adapter { .. } | Command::Encoder { .. } => {
            return Err(HippoError::Unexpected("command reached the store".into()))
        }
        Command::Query { text, k, json } => {
            let hits = store.query(&text, k)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&hits).map_err(HippoError::unexpected)?
                );
            } else {
                for hit in hits {
                    println!(
                        "{}\t{}\t{:.2}\t{}",
                        hit.id, hit.title, hit.score, hit.snippet
                    );
                }
            }
        }
        Command::Add {
            from_json,
            title,
            keywords,
            tags,
            category,
            scope,
            source,
            reason,
            confidence,
            force,
            body,
            json,
        } => {
            let _lock = store.lock_writes()?;
            let mut draft = if from_json {
                draft_from_json(&read_stdin()?)?
            } else {
                NewMemory {
                    title: title.unwrap_or_default(),
                    keywords: split_csv(keywords.as_deref()),
                    body: body.unwrap_or_else(read_stdin_body),
                    tags: split_csv(tags.as_deref()),
                    category,
                    scope: scope.unwrap_or_else(|| GLOBAL_SCOPE.to_string()),
                    memory_id: None,
                    source,
                    reason,
                    confidence,
                }
            };
            draft.source = config::resolve_source(draft.source);
            let memory = store.add(draft, force)?;
            let notices = store.review(&memory)?;
            emit_write(
                &memory.id,
                &memory.path.display().to_string(),
                &notices,
                &write_warnings(&store),
                json,
            )?;
        }
        Command::Update {
            memory_id,
            title,
            keywords,
            tags,
            body,
            scope,
            reason,
            confidence,
            force,
            json,
        } => {
            let _lock = store.lock_writes()?;
            let patch = MemoryPatch {
                title,
                keywords: keywords.map(|value| split_csv(Some(&value))),
                tags: tags.map(|value| split_csv(Some(&value))),
                body,
                scope,
                reason,
                confidence,
            };
            let memory = store.update(&memory_id, patch, force)?;
            let notices = store.review(&memory)?;
            emit_write(
                &memory.id,
                &memory.path.display().to_string(),
                &notices,
                &write_warnings(&store),
                json,
            )?;
        }
        Command::Reindex { all, changed } => {
            let result = store.reindex(changed && !all)?;
            println!(
                "indexed={} added={} updated={} deleted={}",
                result.total(),
                result.added,
                result.updated,
                result.deleted
            );
        }
        Command::Manifest { format } => println!("{}", store.manifest(format == "json")?),
        Command::Digest { format } => println!("{}", store.digest(format == "json")?),
        Command::Consolidate { apply, json } => {
            let _lock = if apply {
                Some(store.lock_writes()?)
            } else {
                None
            };
            let report = store.consolidate(apply)?;
            let warnings = if apply {
                sync_warning(&store).into_iter().collect()
            } else {
                Vec::new()
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report_payload(&report, warnings))
                        .map_err(HippoError::unexpected)?
                );
            } else {
                print_warnings(&warnings)?;
                println!(
                    "duplicates={} contradictions={}",
                    report.duplicates.len(),
                    report.contradictions.len()
                );
                for cluster in &report.duplicates {
                    let absorbed = cluster
                        .members
                        .iter()
                        .filter(|id| *id != &cluster.keeper)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "  keep {} <= {absorbed} ({:.2})",
                        cluster.keeper, cluster.score
                    );
                }
                for finding in &report.contradictions {
                    println!(
                        "  {} <> {} [{}]",
                        finding.a,
                        finding.b,
                        finding.tokens.join(", ")
                    );
                }
                if report.applied {
                    let removed = if report.removed.is_empty() {
                        "-".to_string()
                    } else {
                        report.removed.join(",")
                    };
                    println!("applied removed={removed}");
                }
            }
        }
        Command::Sync { file } => {
            if let Some(file) = file.filter(|file| !store.is_memory_file(file)) {
                println!("skipped: {} is not a memory file", file.display());
                return Ok(());
            }
            let _lock = store.lock_writes()?;
            let report = store.sync()?;
            println!("committed={} pushed={}", report.committed, report.pushed);
        }
        Command::Status { json } => {
            let report = store.status()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(HippoError::unexpected)?
                );
            } else {
                println!(
                    "indexed={} new={} changed={} removed={}",
                    report.indexed,
                    report.new.len(),
                    report.changed.len(),
                    report.removed.len()
                );
                for action in report.actions {
                    println!("  → {action}");
                }
            }
        }
        Command::Show { memory_id } => {
            let path = store.path(&memory_id)?;
            let text = std::fs::read_to_string(path).map_err(HippoError::unexpected)?;
            print!("{text}");
            if !text.ends_with('\n') {
                println!();
            }
        }
        Command::Path { memory_id } => println!("{}", store.path(&memory_id)?.display()),
    }
    Ok(())
}

fn run_encoder(command: EncoderCommand, embed: Option<&str>) -> Result<()> {
    match command {
        EncoderCommand::List => {
            for encoder in Encoder::list() {
                println!("{}\t{}", encoder.name, encoder.describe());
            }
        }
        EncoderCommand::Bench => {
            let name = config::resolve_embed(embed).unwrap_or_else(|| "hash".to_string());
            let encoder = Encoder::parse(&name)?;
            let bench = encoder.bench()?;
            println!(
                "encoder={} load_s={:.3} query_s={:.3} passage_s={:.3}",
                bench.encoder,
                bench.load.as_secs_f64(),
                bench.query.as_secs_f64(),
                bench.passage.as_secs_f64()
            );
        }
    }
    Ok(())
}

fn run_adapter(command: AdapterCommand) -> Result<()> {
    let home = home_dir()?;
    match command {
        AdapterCommand::List => {
            for name in adapter::names()? {
                println!("{name}");
            }
            Ok(())
        }
        AdapterCommand::Install { name } => adapter::install(&name, &home),
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| HippoError::Config("HOME is not set.".into()))
}

fn emit_write(
    id: &str,
    path: &str,
    notices: &[Notice],
    warnings: &[String],
    json: bool,
) -> Result<()> {
    if json {
        let mut payload = json!({"id": id, "path": path});
        if !notices.is_empty() {
            payload["notices"] =
                serde_json::to_value(notices_json(notices)).map_err(HippoError::unexpected)?;
        }
        if !warnings.is_empty() {
            payload["warnings"] = json!(warnings);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(HippoError::unexpected)?
        );
        return Ok(());
    }
    println!("{id}\t{path}");
    let mut stderr = io::stderr().lock();
    for notice in notices {
        writeln!(
            stderr,
            "notice: {} {}: {}",
            notice.kind, notice.other_id, notice.detail
        )
        .map_err(HippoError::unexpected)?;
    }
    drop(stderr);
    print_warnings(warnings)
}

fn print_warnings(warnings: &[String]) -> Result<()> {
    let mut stderr = io::stderr().lock();
    for warning in warnings {
        writeln!(stderr, "warning: {warning}").map_err(HippoError::unexpected)?;
    }
    Ok(())
}

fn write_warnings(store: &MemoryStore) -> Vec<String> {
    let mut warnings = store.capture_warnings.clone();
    warnings.extend(sync_warning(store));
    warnings
}

/// The write already landed, so a failed sync is a warning, not an error.
fn sync_warning(store: &MemoryStore) -> Option<String> {
    store
        .sync()
        .err()
        .map(|err| format!("memory saved but not synced: {err}"))
}

#[derive(Serialize)]
struct NoticeJson {
    kind: String,
    other_id: String,
    detail: String,
}

fn notices_json(notices: &[Notice]) -> Vec<NoticeJson> {
    notices
        .iter()
        .map(|notice| NoticeJson {
            kind: notice.kind.clone(),
            other_id: notice.other_id.clone(),
            detail: notice.detail.clone(),
        })
        .collect()
}

#[derive(Serialize)]
struct ReportJson {
    duplicates: Vec<ClusterJson>,
    contradictions: Vec<ContradictionJson>,
    applied: bool,
    removed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ClusterJson {
    keeper: String,
    members: Vec<String>,
    score: f64,
}

#[derive(Serialize)]
struct ContradictionJson {
    a: String,
    b: String,
    tokens: Vec<String>,
}

fn report_payload(report: &HygieneReport, warnings: Vec<String>) -> ReportJson {
    ReportJson {
        duplicates: report
            .duplicates
            .iter()
            .map(|cluster| ClusterJson {
                keeper: cluster.keeper.clone(),
                members: cluster.members.clone(),
                score: cluster.score,
            })
            .collect(),
        contradictions: report
            .contradictions
            .iter()
            .map(|finding: &Finding| ContradictionJson {
                a: finding.a.clone(),
                b: finding.b.clone(),
                tokens: finding.tokens.clone(),
            })
            .collect(),
        applied: report.applied,
        removed: report.removed.clone(),
        warnings,
    }
}

fn draft_from_json(text: &str) -> Result<NewMemory> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(HippoError::unexpected)?;
    let text_field = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .map(str::to_string)
    };
    let keywords = value
        .get("keywords")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let tags = value
        .get("tags")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(NewMemory {
        title: text_field("title").unwrap_or_default(),
        keywords,
        body: text_field("body").unwrap_or_default(),
        tags,
        category: text_field("category").unwrap_or_else(|| DEFAULT_CATEGORY.to_string()),
        scope: text_field("scope").unwrap_or_else(|| GLOBAL_SCOPE.to_string()),
        memory_id: text_field("id"),
        source: text_field("source"),
        reason: text_field("reason"),
        confidence: text_field("confidence"),
    })
}

fn split_csv(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn read_stdin() -> Result<String> {
    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .map_err(HippoError::unexpected)?;
    Ok(buffer)
}

fn read_stdin_body() -> String {
    if io::stdin().is_terminal() {
        return String::new();
    }
    read_stdin().unwrap_or_default().trim().to_string()
}
