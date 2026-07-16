//! outrider-index: indexes a source tree into a navigable `SymbolTree`.
//! Scans files (scan), parses symbols via tree-sitter (parse), assembles the
//! folder/file/item hierarchy (index), and annotates with git churn (churn).
//! The resulting `SymbolTree` is the input to outrider-layout's shelf-packer.

pub mod buffer;
pub mod call_graph;
pub mod chunk;
pub mod churn;
pub mod dump;
pub mod index;
pub mod language;
pub mod parse;
pub mod scan;
pub mod type_resolve;
pub mod types;

pub use index::{
    index_repo, index_repo_outcome, index_repo_outcome_with_cache, index_repo_with_progress,
    index_repo_with_progress_outcome, index_repo_with_progress_outcome_cancellable, IndexOutcome,
    IndexProgress,
};
pub use language::SourceLanguage;
pub use types::{dedupe_ids, finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
pub use types::{IndexedFile, ParsedFile};
