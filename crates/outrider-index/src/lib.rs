//! outrider-index: indexes a source tree into a navigable `SymbolTree`.
//! Scans files (scan), parses symbols via tree-sitter (parse), assembles the
//! folder/file/item hierarchy (index), and annotates with git churn (churn).
//! The resulting `SymbolTree` is the input to outrider-layout's shelf-packer.

pub mod buffer;
pub mod chunk;
pub mod churn;
pub mod dump;
pub mod index;
pub mod parse;
pub mod scan;
pub mod types;

pub use index::index_repo;
pub use types::{dedupe_ids, finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
