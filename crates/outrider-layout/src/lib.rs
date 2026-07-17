//! Treemap layout engine for outrider-ide.
//! Shelf-packs a `SymbolTree` into a nested set of world-space rectangles
//! using the algorithm in `pack`. Consumers call `pack(tree, cfg)` and
//! receive a `PackLayout` mapping every `SymbolId` to an absolute `Rect`.

pub mod pack;
mod progressive;
mod skyline;
mod zones;

pub use pack::{pack, PackConfig, PackLayout, Rect};
pub use progressive::{pack_progressive, PackCancelled, PackProgress};
