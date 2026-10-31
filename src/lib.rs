pub mod adapter;
pub mod api;
pub mod capture;
pub mod cli;
pub mod config;
pub mod embed;
pub mod encoder;
pub mod error;
pub mod frontmatter;
pub mod hygiene;
pub mod index;
pub mod ranking;
pub mod render;
pub mod search;
pub mod store;
pub mod vcs;

pub use api::MemoryStore;
pub use error::{HippoError, Result};
