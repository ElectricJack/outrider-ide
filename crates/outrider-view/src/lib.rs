//! View-primitives framework for outrider-ide.
//! Defines `ViewSpec` (the JSON document), set/metric/relation resolution,
//! layer composition, commands, and validation. GPUI-free — usable from
//! the CLI and headless tests.

pub mod camera;
pub mod command;
pub mod deps;
pub mod git;
pub mod layers;
pub mod metric;
pub mod partition;
pub mod relation;
pub mod resolve;
pub mod set;
pub mod space;
pub mod spec;
pub mod symbol_id;
pub mod validate;

pub use deps::Deps;
pub use resolve::{ResolveCtx, ResolvedView, SessionState, ViewResolver};
pub use spec::*;
