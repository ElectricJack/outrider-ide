//! Treemap layout engine for outrider-ide.
//! Shelf-packs a `SymbolTree` into a nested set of world-space rectangles
//! using the algorithm in `pack`. Consumers call `pack(tree, cfg)` and
//! receive a `PackLayout` mapping every `SymbolId` to an absolute `Rect`.

pub mod pack;
#[allow(dead_code)] // Private skyline geometry is consumed by the folder packing task.
mod skyline;
#[allow(dead_code)] // Private role profiles are consumed by the semantic packing task.
mod zones;

pub use pack::{pack, PackConfig, PackLayout, Rect};
