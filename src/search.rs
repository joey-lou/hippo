use std::collections::HashMap;

use chrono::NaiveDate;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::config::GLOBAL_SCOPE;
use crate::embed;
use crate::error::{HippoError, Result};
use crate::ranking;

const WEIGHTS: (f64, f64, f64) = (3.0, 5.0, 1.0);
const BM25_MISS: f64 = 8.0;
/// Raw BM25 at least this negative is a strong keyword hit.
const STRONG_BM25: f64 = -8.0;
const NEIGHBORS: usize = 8;
const MIN_COSINE: f64 = 0.15;
const SNIPPET_WORDS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Recall {
    /// Hashing vectors blended into BM25.
    Hash,
    /// Keyword score only.
    Off,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub id: String,
    pub path: String,
    pub title: String,
    pub score: f64,
    pub snippet: String,
    pub scope: String,
}

struct Row {
    id: String,
    path: String,
    title: String,
    scope: String,
    updated: String,
    hits: i64,
    bm25: f64,
    snippet: String,
}

pub fn query(
    conn: &Connection,
    text: &str,
    k: usize,
    scope: &str,
    now: NaiveDate,
    recall: Recall,
) -> Result<Vec<Hit>> {
    let match_query = match_query(text)?;
    let keywords = keyword_rows(conn, &match_query, scope)?;
    let (cosines, neighbors) = if recall == Recall::Off {
        (HashMap::new(), Vec::new())
    } else {
        neighbor_rows(conn, &embed::embed(text), scope)?
    };

    let mut rows: HashMap<String, Row> = HashMap::new();
    for row in keywords {
        rows.insert(row.id.clone(), row);
    }
    for mut row in neighbors {
        if rows.contains_key(&row.id) {
            continue;
        }
        row.bm25 = BM25_MISS;
        row.snippet = body_snippet(conn, &row.id)?;
        rows.insert(row.id.clone(), row);
    }

    let mut hits: Vec<Hit> = rows
        .into_values()
        .map(|row| {
            let cosine = if recall == Recall::Off {
                0.0
            } else {
                cosines.get(&row.id).copied().unwrap_or(0.0).max(0.0)
            };
            Hit {
                score: ranking::adjust(
                    row.bm25,
                    scope != GLOBAL_SCOPE && row.scope == scope,
                    &row.updated,
                    row.hits,
                    now,
                    ranking::EMBED_WEIGHT * cosine,
                ),
                id: row.id,
                path: row.path,
                title: row.title,
                snippet: row.snippet,
                scope: row.scope,
            }
        })
        .collect();
    hits.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    Ok(hits)
}

/// Rank by cosine. A strong keyword hit stays ahead of a vector-only hit.
/// Score stays lower-is-better: `-cosine`, minus 1 when BM25 is below [`STRONG_BM25`].
pub fn query_neural(
    conn: &Connection,
    text: &str,
    k: usize,
    scope: &str,
    encoder: &str,
    query_vec: &[f64],
) -> Result<Vec<Hit>> {
    let match_query = match_query(text)?;
    let keywords = keyword_rows(conn, &match_query, scope)?;
    let mut bm25 = HashMap::new();
    for row in &keywords {
        bm25.insert(row.id.clone(), row.bm25);
    }

    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.path, m.title, m.scope, m.updated, m.hits, v.vector
             FROM memory_vectors v
             JOIN memories m ON m.id = v.id AND m.content_hash = v.content_hash
             WHERE v.encoder = ?1 AND (m.scope = ?2 OR m.scope = ?3)",
        )
        .map_err(HippoError::unexpected)?;
    let stored = stmt
        .query_map(rusqlite::params![encoder, GLOBAL_SCOPE, scope], |row| {
            Ok((
                Row {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    title: row.get(2)?,
                    scope: row.get(3)?,
                    updated: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    hits: row.get(5)?,
                    bm25: 0.0,
                    snippet: String::new(),
                },
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(HippoError::unexpected)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(HippoError::unexpected)?;

    let mut rows: HashMap<String, Row> = HashMap::new();
    for row in keywords {
        rows.insert(row.id.clone(), row);
    }
    let mut cosines = HashMap::new();
    for (mut row, raw) in stored {
        let cosine = cosine_of(query_vec, raw.as_deref()).max(0.0);
        cosines.insert(row.id.clone(), cosine);
        if let Some(existing) = rows.get_mut(&row.id) {
            existing.bm25 = bm25.get(&row.id).copied().unwrap_or(existing.bm25);
            continue;
        }
        row.bm25 = BM25_MISS;
        row.snippet = body_snippet(conn, &row.id)?;
        rows.insert(row.id.clone(), row);
    }

    let mut hits: Vec<Hit> = rows
        .into_values()
        .map(|row| {
            let cosine = cosines.get(&row.id).copied().unwrap_or(0.0);
            let mut score = -cosine;
            if row.bm25 < STRONG_BM25 {
                score -= 1.0;
            }
            Hit {
                score,
                id: row.id,
                path: row.path,
                title: row.title,
                snippet: row.snippet,
                scope: row.scope,
            }
        })
        .collect();
    hits.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    hits.truncate(k);
    Ok(hits)
}

fn cosine_of(query_vec: &[f64], raw: Option<&str>) -> f64 {
    let Some(raw) = raw.filter(|text| !text.is_empty()) else {
        return 0.0;
    };
    let Ok(data) = serde_json::from_str::<Vec<f64>>(raw) else {
        return 0.0;
    };
    if data.len() != query_vec.len() {
        return 0.0;
    }
    embed::cosine(query_vec, &data)
}

fn match_query(text: &str) -> Result<String> {
    let pattern = Regex::new(r"\w+").expect("token pattern");
    let tokens: Vec<String> = pattern
        .find_iter(&text.to_lowercase())
        .map(|item| format!("\"{}\"", item.as_str()))
        .collect();
    if tokens.is_empty() {
        return Err(HippoError::Validation(
            "Query is empty after tokenization.".into(),
        ));
    }
    Ok(tokens.join(" OR "))
}

fn keyword_rows(conn: &Connection, match_query: &str, scope: &str) -> Result<Vec<Row>> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.path, m.title, m.scope, m.updated, m.hits,
                    bm25(memories_fts, ?1, ?2, ?3) AS score,
                    snippet(memories_fts, 2, '', '', '…', 12) AS snip
             FROM memories_fts
             JOIN memories m ON m.id = memories_fts.id
             WHERE memories_fts MATCH ?4
               AND (m.scope = ?5 OR m.scope = ?6)",
        )
        .map_err(HippoError::unexpected)?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                WEIGHTS.0,
                WEIGHTS.1,
                WEIGHTS.2,
                match_query,
                GLOBAL_SCOPE,
                scope
            ],
            |row| {
                Ok(Row {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    title: row.get(2)?,
                    scope: row.get(3)?,
                    updated: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    hits: row.get(5)?,
                    bm25: row.get(6)?,
                    snippet: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                })
            },
        )
        .map_err(HippoError::unexpected)?;
    rows.collect::<std::result::Result<_, _>>()
        .map_err(HippoError::unexpected)
}

fn neighbor_rows(
    conn: &Connection,
    query_vec: &[f64],
    scope: &str,
) -> Result<(HashMap<String, f64>, Vec<Row>)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, title, scope, updated, hits, embedding
             FROM memories
             WHERE scope = ?1 OR scope = ?2",
        )
        .map_err(HippoError::unexpected)?;
    let stored = stmt
        .query_map(rusqlite::params![GLOBAL_SCOPE, scope], |row| {
            Ok((
                Row {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    title: row.get(2)?,
                    scope: row.get(3)?,
                    updated: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    hits: row.get(5)?,
                    bm25: 0.0,
                    snippet: String::new(),
                },
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(HippoError::unexpected)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(HippoError::unexpected)?;

    let mut cosines = HashMap::new();
    let mut ranked = Vec::new();
    for (row, raw) in stored {
        let cosine = embed::cosine(query_vec, &decode_embedding(raw.as_deref()));
        cosines.insert(row.id.clone(), cosine);
        if cosine > MIN_COSINE {
            ranked.push((cosine, row));
        }
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.id.cmp(&right.1.id))
    });
    ranked.truncate(NEIGHBORS);
    Ok((cosines, ranked.into_iter().map(|(_, row)| row).collect()))
}

fn decode_embedding(raw: Option<&str>) -> Vec<f64> {
    let Some(raw) = raw.filter(|text| !text.is_empty()) else {
        return vec![0.0; embed::DIM];
    };
    let Ok(data) = serde_json::from_str::<Vec<f64>>(raw) else {
        return vec![0.0; embed::DIM];
    };
    if data.len() != embed::DIM {
        return vec![0.0; embed::DIM];
    }
    data
}

fn body_snippet(conn: &Connection, memory_id: &str) -> Result<String> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body FROM memories_fts WHERE id = ?1",
            [memory_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(HippoError::unexpected)?;
    let Some(body) = body.filter(|text| !text.is_empty()) else {
        return Ok(String::new());
    };
    let words: Vec<&str> = body.split_whitespace().collect();
    if words.len() <= SNIPPET_WORDS {
        return Ok(words.join(" "));
    }
    Ok(format!("{}…", words[..SNIPPET_WORDS].join(" ")))
}
