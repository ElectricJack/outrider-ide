# Semantic Zones and Skyline Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve Outrider's real folder hierarchy while grouping each folder's immediate children into invisible role-based zones and skyline-packing those zones more densely.

**Architecture:** Add a bottom-up semantic-profile pass and a deterministic fixed-rectangle skyline primitive as private `outrider-layout` modules. `pack.rs` keeps recursive measurement and absolute positioning, routes only real folder containers through a two-level role-block skyline layout, and leaves every non-folder container on the existing multi-candidate shelf algorithm.

**Tech Stack:** Rust 2021, `outrider-index` tree types, `BTreeMap`, built-in unit tests, Cargo.

## Global Constraints

- Design authority: `docs/superpowers/specs/2026-07-16-semantic-zones-skyline-packing-design.md`.
- Preserve `pack(&SymbolTree, &PackConfig) -> PackLayout` and every existing public type and field.
- Never add synthetic nodes or output rectangles for semantic zones.
- Never move a node across a real folder boundary.
- Apply skyline packing only to `SymbolKind::Folder` containers.
- Preserve existing non-folder shelf and source-order behavior.
- Never rotate or resize child rectangles.
- Use only deterministic total ordering and constant candidate-width factors.
- Add no dependencies, settings, serialization, or renderer changes.
- Follow RED-GREEN-REFACTOR for every production behavior.

---

### Task 1: Semantic role profiles

**Files:**
- Create: `crates/outrider-layout/src/zones.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Consumes: `outrider_index::{SymbolId, SymbolKind, SymbolNode}`.
- Produces: `SemanticRole`, `RoleProfile`, `RoleProfiles`, `build_profiles`, and `effective_role`, all `pub(crate)`.
- `RoleProfile { role, strong }` separates explicit/dominant classifications from weak default source classification.

- [ ] **Step 1: Declare the private module and write failing classification tests**

Add `mod zones;` to `lib.rs`. Create `zones.rs` with imports, type declarations, and tests first:

```rust
use std::collections::BTreeMap;
use outrider_index::{SymbolId, SymbolKind, SymbolNode};

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SemanticRole {
    Source,
    Test,
    Example,
    ShaderAsset,
    Docs,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RoleProfile {
    pub role: SemanticRole,
    pub strong: bool,
}

pub(crate) type RoleProfiles = BTreeMap<SymbolId, RoleProfile>;

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    fn node(kind: SymbolKind, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: name.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    #[test]
    fn explicit_folder_roles_cover_all_zones() {
        let cases = [
            ("src", SemanticRole::Source),
            ("unit_tests", SemanticRole::Test),
            ("Demo-Samples", SemanticRole::Example),
            ("ray_shaders", SemanticRole::ShaderAsset),
            ("Documentation", SemanticRole::Docs),
            ("third_party", SemanticRole::Generated),
        ];
        for (name, expected) in cases {
            assert_eq!(explicit_folder_role(name), Some(expected), "{name}");
        }
    }

    #[test]
    fn generated_precedence_wins_ambiguous_names() {
        assert_eq!(
            explicit_folder_role("generated_test_assets"),
            Some(SemanticRole::Generated)
        );
    }

    #[test]
    fn file_roles_cover_suffixes_and_extensions() {
        let cases = [
            ("mesh_test.cpp", SemanticRole::Test),
            ("lighting.demo.rs", SemanticRole::Example),
            ("closest_hit.rchit", SemanticRole::ShaderAsset),
            ("README.md", SemanticRole::Docs),
            ("ordinary.cpp", SemanticRole::Source),
        ];
        for (name, expected) in cases {
            assert_eq!(classify_file(name).role, expected, "{name}");
        }
    }
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p outrider-layout zones::tests`

Expected: compilation fails because `explicit_folder_role`, `classify_file`, and the profile functions do not exist.

- [ ] **Step 3: Implement normalization and explicit classification**

Implement exact token/collapsed-name matching and extension tables from design §4. The classifier shape must be:

```rust
const ROLES_BY_PRECEDENCE: [SemanticRole; 6] = [
    SemanticRole::Generated,
    SemanticRole::Test,
    SemanticRole::Example,
    SemanticRole::ShaderAsset,
    SemanticRole::Docs,
    SemanticRole::Source,
];

fn normalized(name: &str) -> (Vec<String>, String) {
    let lower = name.to_ascii_lowercase();
    let tokens = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect();
    let collapsed = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    (tokens, collapsed)
}

fn has_signal(tokens: &[String], collapsed: &str, signals: &[&str]) -> bool {
    signals.iter().any(|signal| {
        tokens.iter().any(|token| token == *signal) || collapsed == *signal
    })
}

fn explicit_folder_role(name: &str) -> Option<SemanticRole> {
    let (tokens, collapsed) = normalized(name);
    ROLES_BY_PRECEDENCE.into_iter().find(|role| match role {
        SemanticRole::Generated => has_signal(&tokens, &collapsed, &[
            "generated", "vendor", "vendors", "thirdparty", "external",
            "extern", "deps", "dependencies",
        ]),
        SemanticRole::Test => has_signal(&tokens, &collapsed, &[
            "test", "tests", "testing", "spec", "specs",
        ]),
        SemanticRole::Example => has_signal(&tokens, &collapsed, &[
            "example", "examples", "demo", "demos", "sample", "samples",
        ]),
        SemanticRole::ShaderAsset => has_signal(&tokens, &collapsed, &[
            "shader", "shaders", "asset", "assets", "resource", "resources",
            "texture", "textures", "model", "models", "media",
        ]),
        SemanticRole::Docs => has_signal(&tokens, &collapsed, &[
            "doc", "docs", "documentation",
        ]),
        SemanticRole::Source => has_signal(&tokens, &collapsed, &[
            "src", "source", "sources", "include", "includes", "lib", "libs", "core",
        ]),
    })
}

fn classify_file(name: &str) -> RoleProfile {
    let (tokens, collapsed) = normalized(name);
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    let role = ROLES_BY_PRECEDENCE.into_iter().find(|role| {
        file_matches(*role, &tokens, &collapsed, extension.as_deref())
    }).unwrap_or(SemanticRole::Source);
    RoleProfile { role, strong: role != SemanticRole::Source }
}
```

`file_matches` must contain the complete exact lists from design §4.2. `Source` returns `false`; the `unwrap_or` supplies weak source.

- [ ] **Step 4: Add failing profile and inheritance tests**

Add tests proving strong explicit folders, 70% non-source descendant majority, ordinary-source weakness, mixed fallback, and contextual inheritance:

```rust
#[test]
fn seventy_percent_non_source_descendants_make_a_strong_folder() {
    let files = (0..7).map(|i| node(SymbolKind::File, &format!("case_{i}_test.cpp"), vec![]))
        .chain((0..3).map(|i| node(SymbolKind::File, &format!("impl_{i}.cpp"), vec![])))
        .collect();
    let folder = node(SymbolKind::Folder, "unit", files);
    let profiles = build_profiles(&folder);
    assert_eq!(profiles[&folder.id], RoleProfile { role: SemanticRole::Test, strong: true });
}

#[test]
fn ordinary_source_folder_is_weak_and_inherits_test_context() {
    let file = node(SymbolKind::File, "ordinary.cpp", vec![]);
    let folder = node(SymbolKind::Folder, "unit", vec![file]);
    let profiles = build_profiles(&folder);
    assert!(!profiles[&folder.id].strong);
    assert_eq!(effective_role(&folder.id, SemanticRole::Test, &profiles), SemanticRole::Test);
}
```

- [ ] **Step 5: Implement the bottom-up profile pass**

Use fixed role-count arrays indexed by `SemanticRole as usize`:

```rust
#[derive(Default)]
struct RoleCounts {
    files: [u64; 6],
}

impl RoleCounts {
    fn add(&mut self, other: &Self) {
        for (left, right) in self.files.iter_mut().zip(other.files) {
            *left += right;
        }
    }

    fn total(&self) -> u64 {
        self.files.iter().sum()
    }

    fn dominant_non_source(&self) -> Option<SemanticRole> {
        let total = self.total();
        ROLES_BY_PRECEDENCE.into_iter()
            .filter(|role| *role != SemanticRole::Source)
            .find(|role| self.files[*role as usize] * 10 >= total * 7 && total > 0)
    }
}

pub(crate) fn build_profiles(root: &SymbolNode) -> RoleProfiles {
    let mut profiles = BTreeMap::new();
    collect_profiles(root, &mut profiles);
    profiles
}

fn collect_profiles(node: &SymbolNode, profiles: &mut RoleProfiles) -> RoleCounts {
    match node.id.kind {
        SymbolKind::File => {
            let profile = classify_file(&node.name);
            profiles.insert(node.id.clone(), profile);
            let mut counts = RoleCounts::default();
            counts.files[profile.role as usize] = 1;
            counts
        }
        SymbolKind::Folder => {
            let mut counts = RoleCounts::default();
            for child in &node.children {
                counts.add(&collect_profiles(child, profiles));
            }
            let profile = explicit_folder_role(&node.name)
                .map(|role| RoleProfile { role, strong: true })
                .or_else(|| counts.dominant_non_source().map(|role| RoleProfile { role, strong: true }))
                .unwrap_or(RoleProfile { role: SemanticRole::Source, strong: false });
            profiles.insert(node.id.clone(), profile);
            counts
        }
        _ => {
            profiles.insert(node.id.clone(), RoleProfile { role: SemanticRole::Source, strong: false });
            RoleCounts::default()
        }
    }
}

pub(crate) fn effective_role(
    id: &SymbolId,
    inherited: SemanticRole,
    profiles: &RoleProfiles,
) -> SemanticRole {
    let profile = profiles.get(id).copied().unwrap_or(RoleProfile {
        role: SemanticRole::Source,
        strong: false,
    });
    if profile.strong { profile.role } else { inherited }
}
```

- [ ] **Step 6: Verify Task 1 and commit**

Run: `cargo fmt -p outrider-layout`

Run: `cargo test -p outrider-layout zones::tests`

Expected: all zone tests pass.

Run: `cargo clippy -p outrider-layout --all-targets --no-deps -- -D warnings`

Expected: no warnings in the changed crate.

Commit: `feat: classify folder children into semantic roles`

---

### Task 2: Deterministic skyline primitive

**Files:**
- Create: `crates/outrider-layout/src/skyline.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Produces `pub(crate) struct SkylineLayout { positions: Vec<(f64, f64)>, bounds: (f64, f64) }`.
- Produces `pub(crate) fn skyline_pack(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> SkylineLayout`.
- Input and output positions have identical indices; the primitive never reorders items.

- [ ] **Step 1: Write failing fixed-width skyline tests**

Declare `mod skyline;` in `lib.rs`. In `skyline.rs`, define the output type and tests:

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkylineLayout {
    pub positions: Vec<(f64, f64)>,
    pub bounds: (f64, f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_width_skyline_fills_a_right_hand_cavity() {
        let packed = pack_at_width(&[(6.0, 4.0), (4.0, 2.0), (4.0, 2.0)], 0.0, 10.0);
        assert_eq!(packed.positions, vec![(0.0, 0.0), (6.0, 0.0), (6.0, 2.0)]);
        assert_eq!(packed.bounds, (10.0, 4.0));
    }

    #[test]
    fn gap_separates_horizontal_and_vertical_neighbors() {
        let packed = pack_at_width(&[(6.0, 4.0), (4.0, 2.0), (4.0, 2.0)], 1.0, 12.0);
        assert_eq!(packed.positions[1], (7.0, 0.0));
        assert_eq!(packed.positions[2], (7.0, 3.0));
    }
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p outrider-layout skyline::tests`

Expected: compilation fails because `pack_at_width` does not exist.

- [ ] **Step 3: Implement skyline segments and one-width placement**

Use this private representation and exact placement order:

```rust
#[derive(Debug, Clone, Copy)]
struct Segment {
    x: f64,
    width: f64,
    height: f64,
}

impl Segment {
    fn end(self) -> f64 { self.x + self.width }
}

fn pack_at_width(sizes: &[(f64, f64)], gap: f64, bin_width: f64) -> SkylineLayout {
    if sizes.is_empty() {
        return SkylineLayout { positions: vec![], bounds: (0.0, 0.0) };
    }
    let mut skyline = vec![Segment { x: 0.0, width: bin_width, height: 0.0 }];
    let mut positions = Vec::with_capacity(sizes.len());
    let (mut used_w, mut used_h) = (0.0_f64, 0.0_f64);

    for &(width, height) in sizes {
        let padded_w = width + gap;
        let padded_h = height + gap;
        let (x, y) = skyline.iter()
            .map(|segment| segment.x)
            .filter(|x| *x + padded_w <= bin_width)
            .map(|x| {
                let y = skyline.iter()
                    .filter(|segment| segment.x < x + padded_w && segment.end() > x)
                    .map(|segment| segment.height)
                    .fold(0.0, f64::max);
                (x, y)
            })
            .min_by(|a, b| {
                (a.1 + padded_h).total_cmp(&(b.1 + padded_h))
                    .then(a.1.total_cmp(&b.1))
                    .then(a.0.total_cmp(&b.0))
            })
            .expect("candidate width is clamped to the widest padded rectangle");

        raise_skyline(&mut skyline, x, padded_w, y + padded_h);
        positions.push((x, y));
        used_w = used_w.max(x + width);
        used_h = used_h.max(y + height);
    }
    SkylineLayout { positions, bounds: (used_w, used_h) }
}

fn raise_skyline(skyline: &mut Vec<Segment>, x: f64, width: f64, height: f64) {
    let end = x + width;
    let mut next = Vec::with_capacity(skyline.len() + 2);
    let mut inserted = false;
    for segment in skyline.iter().copied() {
        if segment.end() <= x {
            next.push(segment);
            continue;
        }
        if segment.x >= end {
            if !inserted {
                next.push(Segment { x, width, height });
                inserted = true;
            }
            next.push(segment);
            continue;
        }
        if segment.x < x {
            next.push(Segment {
                x: segment.x,
                width: x - segment.x,
                height: segment.height,
            });
        }
        if !inserted {
            next.push(Segment { x, width, height });
            inserted = true;
        }
        if segment.end() > end {
            next.push(Segment {
                x: end,
                width: segment.end() - end,
                height: segment.height,
            });
        }
    }
    if !inserted {
        next.push(Segment { x, width, height });
    }

    let mut merged: Vec<Segment> = Vec::with_capacity(next.len());
    for segment in next {
        if let Some(last) = merged.last_mut() {
            if last.end() == segment.x && last.height == segment.height {
                last.width += segment.width;
                continue;
            }
        }
        merged.push(segment);
    }
    *skyline = merged;
}
```

`raise_skyline` must split segments at `x` and `x + width`, replace the covered span with one raised segment, keep segments sorted, and merge adjacent segments whose heights compare equal. Add a unit test asserting the resulting segments for a placement spanning two unequal segments.

- [ ] **Step 4: Write failing multi-candidate tests**

Add tests for single-item natural bounds, repeatability, width clamping, and baseline tie preference:

```rust
#[test]
fn public_skyline_pack_is_repeatable_and_preserves_index_order() {
    let sizes = [(10.0, 8.0), (4.0, 2.0), (4.0, 2.0), (2.0, 6.0)];
    let first = skyline_pack(&sizes, 1.0, 1.6);
    let second = skyline_pack(&sizes, 1.0, 1.6);
    assert_eq!(first, second);
    assert_eq!(first.positions.len(), sizes.len());
}

#[test]
fn single_item_keeps_natural_bounds() {
    assert_eq!(
        skyline_pack(&[(640.0, 58.0)], 8.0, 1.6),
        SkylineLayout { positions: vec![(0.0, 0.0)], bounds: (640.0, 58.0) }
    );
}
```

- [ ] **Step 5: Implement multi-candidate width selection**

Use the factors from the design and evaluate the baseline first:

```rust
const WIDTH_FACTORS: [f64; 9] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.5, 2.0, 3.0];

pub(crate) fn skyline_pack(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> SkylineLayout {
    if sizes.len() <= 1 {
        return SkylineLayout {
            positions: sizes.iter().map(|_| (0.0, 0.0)).collect(),
            bounds: sizes.first().copied().unwrap_or((0.0, 0.0)),
        };
    }
    let widest = sizes.iter().map(|(w, _)| w + gap).fold(0.0, f64::max);
    let padded_area: f64 = sizes.iter().map(|(w, h)| (w + gap) * (h + gap)).sum();
    let baseline = widest.max((padded_area * aspect).sqrt());
    let mut widths = vec![baseline];
    for factor in WIDTH_FACTORS {
        let candidate = widest.max(baseline * factor);
        if !widths.contains(&candidate) {
            widths.push(candidate);
        }
    }
    let mut best = pack_at_width(sizes, gap, widths[0]);
    let mut best_score = aspect_envelope_area(best.bounds, aspect);
    for width in widths.into_iter().skip(1) {
        let candidate = pack_at_width(sizes, gap, width);
        let score = aspect_envelope_area(candidate.bounds, aspect);
        if score.total_cmp(&best_score).is_lt() {
            best = candidate;
            best_score = score;
        }
    }
    best
}

fn aspect_envelope_area((w, h): (f64, f64), aspect: f64) -> f64 {
    let envelope_w = w.max(h * aspect);
    envelope_w * (envelope_w / aspect)
}
```

- [ ] **Step 6: Add overlap/containment and large-input coverage**

For a deterministic 512-item size sequence, call `skyline_pack`; assert every rectangle lies within `bounds`, every pair is separated by at least `gap` on one axis, every coordinate is finite, and positions retain the input count. Do not assert elapsed time.

- [ ] **Step 7: Verify Task 2 and commit**

Run: `cargo fmt -p outrider-layout`

Run: `cargo test -p outrider-layout skyline::tests`

Run: `cargo clippy -p outrider-layout --all-targets --no-deps -- -D warnings`

Expected: all skyline tests pass and the changed crate has no warnings.

Commit: `feat: add deterministic skyline rectangle packing`

---

### Task 3: Two-level semantic zone layout for folders

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs`
- Test: `crates/outrider-layout/src/pack.rs`

**Interfaces:**
- `pack` calls `build_profiles` once and starts recursive sizing with `SemanticRole::Source`.
- `size` receives `inherited_role: SemanticRole` and `profiles: &RoleProfiles`.
- Private `arrange_folder` returns positions aligned with its input children plus occupied bounds.
- Non-folder ordering and shelf placement remain in `pack.rs`.

- [ ] **Step 1: Write a failing hierarchy-preservation and role-locality test**

Add a root fixture containing two source files, two test files, an examples folder, a shader file, and docs. After `pack`, assert the output ID set equals the input tree ID set. Compute one bounding rectangle per role from child rectangles and assert role bounds do not overlap.

The test must also assert that both test children lie in the same role bound even when one is small enough to fit a source-zone cavity.

- [ ] **Step 2: Run the integration test and verify RED**

Run: `cargo test -p outrider-layout folder_semantic_zones_preserve_hierarchy_and_do_not_interleave`

Expected: failure because current folder packing has no role-block isolation.

- [ ] **Step 3: Build profiles once and thread effective context through sizing**

Change `pack` and `size` as follows:

```rust
use crate::skyline::{skyline_pack, SkylineLayout};
use crate::zones::{build_profiles, effective_role, RoleProfiles, SemanticRole};

pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout {
    let profiles = build_profiles(&tree.root);
    let mut rel = BTreeMap::new();
    size(&tree.root, SemanticRole::Source, &profiles, cfg, &mut rel);
    let mut rects = BTreeMap::new();
    absolute(&tree.root, 0.0, 0.0, &rel, &mut rects);
    PackLayout { rects }
}

fn size(
    node: &SymbolNode,
    inherited_role: SemanticRole,
    profiles: &RoleProfiles,
    cfg: &PackConfig,
    rel: &mut BTreeMap<SymbolId, (f64, f64, f64, f64)>,
) -> (f64, f64)
```

Keep the existing leaf branch as the first branch. Replace recursive child measurement with this exact context propagation:

```rust
let folder = matches!(node.id.kind, SymbolKind::Folder);
let measured: Vec<(&SymbolNode, (f64, f64), SemanticRole)> = node.children.iter()
    .map(|child| {
        let role = if folder {
            effective_role(&child.id, inherited_role, profiles)
        } else {
            SemanticRole::Source
        };
        let dimensions = size(child, role, profiles, cfg, rel);
        (child, dimensions, role)
    })
    .collect();
```

Remove `doc_stats`, `doc_rank`, and `name_is_doc` from `pack.rs`; their intended folder-level behavior is superseded and expanded by `zones.rs`. Keep `is_doc_ext` for source-order detection.

Delete the obsolete pack-module tests `doc_rank_files_by_name_folders_by_recursive_share`, `doc_file_sinks_below_source_in_folder`, and `folder_doc_share_over_70_percent_sinks` only after the equivalent file-extension, descendant-threshold, and folder-zone behaviors are green in `zones.rs` and the new folder integration tests.

- [ ] **Step 4: Implement folder grouping and two-level skyline placement**

Introduce a private sized-child record and folder arranger:

```rust
struct FolderChild<'a> {
    node: &'a SymbolNode,
    size: (f64, f64),
    role: SemanticRole,
}

struct FolderArrangement {
    positions: Vec<(SymbolId, f64, f64)>,
    bounds: (f64, f64),
}

fn arrange_folder(children: &mut [FolderChild<'_>], gap: f64, aspect: f64) -> FolderArrangement {
    children.sort_by(|a, b| {
        a.role.cmp(&b.role)
            .then(b.size.1.total_cmp(&a.size.1))
            .then(a.node.name.as_bytes().cmp(b.node.name.as_bytes()))
            .then(a.node.id.ordinal.cmp(&b.node.id.ordinal))
    });

    let mut groups: Vec<(SemanticRole, Vec<&FolderChild<'_>>, SkylineLayout)> = Vec::new();
    for role in [
        SemanticRole::Source, SemanticRole::Test, SemanticRole::Example,
        SemanticRole::ShaderAsset, SemanticRole::Docs, SemanticRole::Generated,
    ] {
        let members: Vec<_> = children.iter().filter(|child| child.role == role).collect();
        if !members.is_empty() {
            let sizes: Vec<_> = members.iter().map(|child| child.size).collect();
            let layout = skyline_pack(&sizes, gap, aspect);
            groups.push((role, members, layout));
        }
    }

    let block_sizes: Vec<_> = groups.iter().map(|(_, _, layout)| layout.bounds).collect();
    let blocks = skyline_pack(&block_sizes, gap, aspect);
    let mut positions = Vec::with_capacity(children.len());
    for ((_, members, layout), (block_x, block_y)) in groups.iter().zip(blocks.positions) {
        for ((child_x, child_y), child) in layout.positions.iter().zip(members) {
            positions.push((
                child.node.id.clone(),
                block_x + child_x,
                block_y + child_y,
            ));
        }
    }
    FolderArrangement { positions, bounds: blocks.bounds }
}
```

In `size`, use `arrange_folder` only when `node.id.kind == SymbolKind::Folder`. Add `cfg.gap` and `cfg.container_header + cfg.gap` offsets when writing child entries to `rel`, then calculate parent dimensions as:

```rust
let wh = (
    arrangement.bounds.0 + 2.0 * cfg.gap,
    cfg.container_header + arrangement.bounds.1 + 2.0 * cfg.gap,
);
```

For every non-folder node, retain the existing source/kind sorting and multi-candidate shelf placement.

- [ ] **Step 5: Run focused tests and reconcile exact folder expectations**

Run: `cargo test -p outrider-layout pack::tests`

Expected: the new semantic-zone test passes. Existing exact tests for folder coordinates may fail because folder geometry intentionally changes; inspect and update exact coordinates only. Do not weaken determinism, ordering, containment, or source-order assertions.

- [ ] **Step 6: Add a non-folder source-order regression**

Construct one C++ `File` node with three differently sized children in byte order. Record their positions before the folder integration change (using commit `HEAD~1` if needed) and assert the same relative positions after integration. The test name is `folder_skyline_does_not_change_cpp_file_layout`.

- [ ] **Step 7: Verify Task 3 and commit**

Run: `cargo fmt -p outrider-layout`

Run: `cargo test -p outrider-layout`

Run: `cargo clippy -p outrider-layout --all-targets --no-deps -- -D warnings`

Expected: all layout tests pass with no changed-crate warnings.

Commit: `feat: skyline-pack semantic zones within folders`

---

### Task 4: Density acceptance and full verification

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs` tests
- Modify: `crates/outrider-layout/src/skyline.rs` tests if a geometry defect is exposed
- Modify: `crates/outrider-layout/src/zones.rs` tests if a classification defect is exposed

**Interfaces:**
- No new production interfaces.
- Acceptance compares current folder output with the retained private shelf helpers using identical fixed child sizes.

- [ ] **Step 1: Write the screenshot-shaped density acceptance test**

Create a root with one large nested source project, one medium source project, and at least eight small role-varied projects matching the proportions in the original screenshot. Use the resulting child sizes to calculate the old multi-candidate shelf bounds. Assert:

```rust
let packed = pack(&tree, &cfg());
let root = rect(&packed, "");
let skyline_content = (
    root.w - 2.0 * cfg().gap,
    root.h - cfg().container_header - 2.0 * cfg().gap,
);
let shelf_target = choose_target_height(&child_sizes, cfg().gap, cfg().aspect);
let shelf_content = shelf_bounds(&child_sizes, cfg().gap, shelf_target);
assert!(
    aspect_envelope_area(skyline_content, cfg().aspect)
        < aspect_envelope_area(shelf_content, cfg().aspect)
);
```

The test must fail if folder routing is reverted to shelf packing.

- [ ] **Step 2: Add generic containment/no-overlap helpers**

For each non-leaf node in the acceptance tree, assert every immediate child lies within the parent's content region and every pair of immediate children is separated on at least one axis. Assert `packed.rects.len()` equals the recursive node count.

- [ ] **Step 3: Run the complete layout suite**

Run: `cargo test -p outrider-layout`

Expected: all semantic, skyline, integration, density, determinism, containment, and legacy tests pass.

- [ ] **Step 4: Run repository verification**

Run: `cargo fmt -p outrider-layout -- --check`

Expected: success with no diff.

Run: `cargo clippy -p outrider-layout --all-targets --no-deps -- -D warnings`

Expected: success with no changed-crate warnings. Record any pre-existing dependency lint separately without changing unrelated crates.

Run: `cargo test --workspace`

Expected: all workspace tests pass. Existing unrelated warnings may remain, but no new warnings may originate in `outrider-layout`.

Run: `git diff --check`

Expected: success with no whitespace errors.

- [ ] **Step 5: Request independent code review**

Review the complete branch against the design spec, specifically checking role inheritance, strong/weak precedence, skyline segment splitting, gap correctness, role-block isolation, deterministic ties, folder-only routing, and acceptance-test strength. Fix all Critical and Important findings through new RED-GREEN cycles.

- [ ] **Step 6: Commit final acceptance coverage**

Commit: `test: verify semantic skyline packing density`

- [ ] **Step 7: Finish the development branch**

Use `superpowers:verification-before-completion`, rerun fresh verification, then use `superpowers:finishing-a-development-branch` to offer merge, PR, keep, or discard options.
