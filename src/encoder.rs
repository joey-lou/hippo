//! Which vector to blend into search.
//!
//! `hash` and `off` need no download. A named model downloads its weights on
//! first use into `~/.cache/hippo/models` and stores one vector per note.
//! Switching models does not rewrite the markdown.

use std::path::PathBuf;
use std::time::Instant;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use crate::error::{HippoError, Result};

const BENCH_QUERY: &str = "where do the notes say to look";
const BENCH_PASSAGE: &str =
    "A short note about flights, banks, and a dictionary entry. 多音字 stays in the text.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoder {
    pub name: &'static str,
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Off,
    Hash,
    Neural {
        model: EmbeddingModel,
        max_length: usize,
    },
}

impl Encoder {
    pub fn is_neural(&self) -> bool {
        matches!(self.kind, Kind::Neural { .. })
    }

    pub fn parse(name: &str) -> Result<Self> {
        CATALOG
            .iter()
            .find(|encoder| encoder.name == name)
            .cloned()
            .ok_or_else(|| {
                HippoError::Validation(format!(
                    "unknown encoder '{name}'. available: {}",
                    CATALOG
                        .iter()
                        .map(|encoder| encoder.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    pub fn list() -> &'static [Encoder] {
        CATALOG
    }

    pub fn describe(&self) -> &'static str {
        match &self.kind {
            Kind::Off => "Keyword search only.",
            Kind::Hash => "Hashing embedding. No download. The default.",
            Kind::Neural {
                model: EmbeddingModel::BGESmallENV15Q,
                ..
            } => "Quantized BGE small, English, 384 numbers, 512 tokens.",
            Kind::Neural {
                model: EmbeddingModel::BGEBaseENV15Q,
                ..
            } => "Quantized BGE base, English, 768 numbers, 512 tokens.",
            Kind::Neural {
                model: EmbeddingModel::BGEM3,
                ..
            } => "BGE-M3, 100+ languages, 1024 numbers. Hippo sends at most 2048 tokens.",
            Kind::Neural { .. } => "Neural embedding model.",
        }
    }

    pub fn load_model(&self) -> Result<TextEmbedding> {
        let Kind::Neural { model, max_length } = self.kind.clone() else {
            return Err(HippoError::Unexpected(
                "this encoder has no model to load".into(),
            ));
        };
        let cache = cache_dir()?;
        std::fs::create_dir_all(&cache).map_err(HippoError::unexpected)?;
        TextEmbedding::try_new(
            TextInitOptions::new(model)
                .with_cache_dir(cache)
                .with_max_length(max_length)
                .with_show_download_progress(true),
        )
        .map_err(HippoError::unexpected)
    }

    pub fn embed(&self, model: &mut TextEmbedding, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if !self.is_neural() {
            return Err(HippoError::Unexpected(
                "this encoder has no model to run".into(),
            ));
        }
        model.embed(texts, Some(1)).map_err(HippoError::unexpected)
    }

    /// In-process timings. Process startup is not included.
    pub fn bench(&self) -> Result<Bench> {
        if !self.is_neural() {
            if matches!(self.kind, Kind::Off) {
                return Ok(Bench {
                    encoder: self.name,
                    load: std::time::Duration::ZERO,
                    query: std::time::Duration::ZERO,
                    passage: std::time::Duration::ZERO,
                });
            }
            let started = Instant::now();
            let _ = crate::embed::embed(BENCH_QUERY);
            let query = started.elapsed();
            let started = Instant::now();
            let _ = crate::embed::embed(BENCH_PASSAGE);
            return Ok(Bench {
                encoder: self.name,
                load: std::time::Duration::ZERO,
                query,
                passage: started.elapsed(),
            });
        }
        let started = Instant::now();
        let mut model = self.load_model()?;
        let load = started.elapsed();
        let started = Instant::now();
        self.embed(&mut model, &[BENCH_QUERY])?;
        let query = started.elapsed();
        let started = Instant::now();
        self.embed(&mut model, &[BENCH_PASSAGE])?;
        Ok(Bench {
            encoder: self.name,
            load,
            query,
            passage: started.elapsed(),
        })
    }
}

#[derive(Debug)]
pub struct Bench {
    pub encoder: &'static str,
    pub load: std::time::Duration,
    pub query: std::time::Duration,
    pub passage: std::time::Duration,
}

const CATALOG: &[Encoder] = &[
    Encoder {
        name: "hash",
        kind: Kind::Hash,
    },
    Encoder {
        name: "off",
        kind: Kind::Off,
    },
    Encoder {
        name: "bge-small",
        kind: Kind::Neural {
            model: EmbeddingModel::BGESmallENV15Q,
            max_length: 512,
        },
    },
    Encoder {
        name: "bge-base",
        kind: Kind::Neural {
            model: EmbeddingModel::BGEBaseENV15Q,
            max_length: 512,
        },
    },
    Encoder {
        name: "bge-m3",
        kind: Kind::Neural {
            model: EmbeddingModel::BGEM3,
            max_length: 2048,
        },
    },
];

fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(home.join(".cache/hippo/models"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_resolve() {
        assert!(Encoder::parse("hash").unwrap().name == "hash");
        assert!(!Encoder::parse("hash").unwrap().is_neural());
        assert!(Encoder::parse("bge-m3").unwrap().is_neural());
        assert!(Encoder::parse("nope").is_err());
    }
}
