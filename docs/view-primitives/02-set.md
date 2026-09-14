# 02 — Set: expressions, resolution, live sets, partitions

**Parent:** [../view-primitives.md](../view-primitives.md) §2.2, §3.4, §5 (SetExpr forms), §8.2
**Depends on:** [00-framework.md](00-framework.md) (`SetRef`, `Deps`, `ResolveCtx`, `SessionState`, `ViewResolver`, `TreeIndex` move, `symbol_id.rs`); references [03-fill.md](03-fill.md) (`MetricRegistry`, `MetricProvider`) for `where` and [05-edges.md](05-edges.md) (`RelationRegistry`, `RelationProvider`) for `reach`.
**Deliverable:** `SetExpr` (serde) in `spec.rs`; `crates/outrider-view/src/set.rs` (`ResolvedSet`, `SetResolver`, static dependency analysis); `partition.rs` (`PartitionRegistry`, `PartitionRef`); `git.rs` (`changed(...)` evaluation + a pure `GitProbe`); the app-side git watcher `crates/outrider/src/view/git_watch.rs`; the two-pass `visible()` flow in `paint_items`. Sets paint nothing; every other layer takes a `SetRef`.

---

## 1. Purpose and scope

A Set is a `HashSet<SymbolId>` plus optional byte ranges, produced from a `SetExpr` (parent §2.2). This spec defines every expression form in parent §5, how each resolves against the tree/layout/session, how live sets invalidate (parent §3.4), the session pseudo-ids `$focus/$hover/$selection`, bare-path resolution, and the two data providers that only sets and the Space consume: `PartitionRegistry` (for `community`, `layer`, and 01-space `regroup`) and the git probe (for `changed`, `Deps::GIT`).

The **Milestone-0 subset** (00-framework §5 step 1) is called out in §4.9: `ids` (with pseudo-ids, wire ids, bare paths), `neighbors`, `union`. Everything else may land in later milestones (parent §9: 2 for `glob`/`kind`, 5 for `fuzzy`, 6 for `reach`/`changed`/`where`, 8 for partitions) without changing the types defined here.

---

## 2. Ground truth: existing code touched

| File | Symbol | ~Line | What it does today | What changes |
|---|---|---|---|---|
| `crates/outrider-index/src/types.rs` | `SymbolId { kind, qualified_path, ordinal }`, `SymbolKind::{Folder, File, Chunk, Item{label}}`, `label()`, `SymbolNode { byte_range, name, children, .. }`, `SymbolTree { root, repo_root }` | 13–72 | The keys and the tree. | Unchanged. |
| `crates/outrider/src/focus.rs` → `crates/outrider-index/src/tree_index.rs` | `TreeIndex { nodes, parents }`, `node/parent/depth` | 12–59 | id → node, parent. | Moved per 00-framework §2.8; this spec uses `iter()` and adds `children(id)` (via `node(id).children`) — no new storage. |
| `crates/outrider/src/focus.rs` | `neighbors(current, pack, index) -> [Option<SymbolId>;4]`, `spatial_step` | 152–220 | Arrow targets, cached in `TreemapView.neighbors` (treemap.rs:531, refreshed at 1448–1456). | Unchanged; the app passes the cached array through `SessionState.neighbors`. |
| `crates/outrider/src/palette.rs` | `fuzzy_match(query, name) -> bool` | 118–130 | Subsequence match, lowercase. | **Moved** to `crates/outrider-index/src/search.rs` (`pub fn fuzzy_match`), `pub use outrider_index::search::fuzzy_match;` left in `palette.rs`. |
| `crates/outrider/src/treemap.rs` | `paint_items` | 1433–1807 | Builds `DrawItem`s at L1483 (`world::visible_nodes`), then `PaintItem`s. | Two-pass resolve around L1483 (§5.2). |
| `crates/outrider/src/treemap.rs` | `render` | 4064–4070 | `advance_layout_transition`, `poll_loading` each frame. | Poll the git watcher here (§5.3). |
| `crates/outrider/src/treemap.rs` | `on_mouse_move` (`hover_id = hit`) | 2029–2056 | Sets hover. | `view_dirty |= HOVER` (00-framework §3.4; listed for completeness). |
| `crates/outrider/src/buffers.rs` | `BufferManager::file_path_of(qualified_path)`, `Materialized.buffer.byte_to_line` | 50–53, 25–28 | Path-part helper; rope line index. | `file_path_of` logic duplicated in `outrider-view` as `set::file_part(&str)` (also exists in `call_graph.rs` and pack.rs `file_ext`); `changed()` builds its own line table (§4.4) — it must not depend on the app's buffers. |
| `crates/outrider-index/src/churn.rs` | `git_command`, `git_stdout`, `git_head` | 198–251 | `git -C <root>` with `LC_ALL=C`. | Make `git_command`/`git_stdout` `pub` (in `churn.rs` or a new `git_util.rs`) so `outrider-view/src/git.rs` reuses them; `git_head` stays private (the probe reads files, §4.4). |
| `crates/outrider-index/src/call_graph.rs` | `resolve_calls(&SymbolId, &SymbolTree) -> CallGraphData` | 21 | Only relation today. | Consumed through `RelationRegistry` (05-edges); `reach` never calls it directly. |
| `crates/outrider-layout/src/pack.rs` | `PackLayout { rects: BTreeMap<SymbolId, Rect> }` | 49–53 | Laid-out set = `rects.keys()`. | Read by `not` and as the `where` domain. |
| `crates/outrider-index/tests/common/mod.rs`, `churn_test.rs` | `copy_fixture("mini_repo")`, `git_fixture()` | — | Temp-dir fixture + `git init/commit` helper. | Copied into `crates/outrider-view/tests/common/mod.rs` (fixture path via `env!("CARGO_MANIFEST_DIR")/../outrider-index/tests/fixtures`). |

---

## 3. Spec types

All in `crates/outrider-view/src/spec.rs` (00-framework §2.2). Externally tagged, camelCase variant names → exactly one key per object as in parent §5.

### 3.1 `SetExpr`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum SetExpr {
    /// {"ref": "hidden"} — a named set from ViewSpec.sets
    Ref(String),
    /// {"ids": ["fn:src/a.rs::foo", "src/b.rs", "$focus"]} — wire ids, bare paths, pseudo-ids (§4.1)
    Ids(Vec<String>),
    /// {"glob": "src/auth/**"} — globset over qualified_path (§4.2)
    Glob(String),
    /// {"kind": "fn"} | {"kind": ["fn","struct"]} — SymbolKind label or alias (§4.2)
    Kind(OneOrMany<String>),
    /// {"fuzzy": "pars itm"} — palette-style subsequence match on node.name (§4.2)
    Fuzzy(String),
    /// {"where": {"metric": "churn", "op": ">", "value": "p90"}} (§4.3)
    Where(WhereExpr),
    /// {"reach": {"from": {...}, "relation": "calls", "direction": "in", "depth": 2}} (§4.3)
    Reach(ReachExpr),
    /// {"changed": "HEAD~1"} | "a..b" | "worktree" (§4.4)
    Changed(String),
    /// {"community": {"partition": "communities", "id": "3"}} (§4.7)
    Community(PartitionMember),
    /// {"layer": {"partition": "layers", "name": "domain"}} (§4.7)
    Layer(PartitionMember),
    Children(Box<SetExpr>), Ancestors(Box<SetExpr>), Descendants(Box<SetExpr>), FileOf(Box<SetExpr>),
    /// {"neighbors": "focus"} — the four arrow targets (§4.6)
    Neighbors(NeighborsOf),
    /// {"visible": true} — this frame's DrawItems (§4.6)
    Visible(bool),
    Union(Vec<SetExpr>), Intersect(Vec<SetExpr>),
    /// {"minus": [a, b]} — exactly two operands (validated)
    Minus(Vec<SetExpr>),
    /// {"not": x} — relative to the laid-out set (§4.5)
    Not(Box<SetExpr>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
#[serde(untagged)] pub enum OneOrMany<T> { One(T), Many(Vec<T>) }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")] pub enum NeighborsOf { Focus }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhereExpr { pub metric: String, pub op: CmpOp, pub value: WhereValue }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum CmpOp { #[serde(rename = ">")] Gt, #[serde(rename = ">=")] Ge, #[serde(rename = "<")] Lt,
                 #[serde(rename = "<=")] Le, #[serde(rename = "==")] Eq, #[serde(rename = "!=")] Ne }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)] pub enum WhereValue { Percentile(String) /* "p0".."p100" */, Number(f64) }
impl Hash for WhereValue { /* Percentile → str hash; Number → f64::to_bits */ }
impl Eq for WhereValue {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachExpr { pub from: Box<SetExpr>, pub relation: String,
                       #[serde(default)] pub direction: Direction, #[serde(default)] pub depth: Depth }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "camelCase")] pub enum Direction { #[default] Out, In, Both }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(untagged)] pub enum Depth { N(u32), Inf(InfWord) }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")] pub enum InfWord { Inf }
impl Default for Depth { fn default() -> Self { Depth::N(1) } }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartitionMember { pub partition: String, #[serde(alias = "name")] pub id: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartitionRef { pub partition: String }          // used by SpaceSpec.regroup (01-space §3)
```

`SetRef` (00-framework §2.2) is `Name(String) | Inline(Box<SetExpr>)`, untagged; a bare string in a layer field is a name, an object is inline. `Hash` on `SetExpr` is the structural hash the resolver memoises by (§4.0).

Validation (`validate.rs`): unknown `Ref` name → hard; `Ref` cycle → hard (`"sets: cycle hidden → a → hidden"`); `Minus` with `len != 2` → hard; `Where.value` percentile not `p0..=p100` → hard; `Reach.relation` unknown → hard (via `known_relations`); `Where.metric` unknown → hard (via `known_metrics`); `Community/Layer.partition` unknown → soft (registry not available to `validate`, checked at resolve). `Visible/Neighbors/$focus/$hover/$selection` in a saved file → soft "live set" note.

### 3.2 `ResolvedSet`

```rust
pub struct ResolvedSet {
    pub ids: HashSet<SymbolId>,
    /// Anchor ranges (bytes, relative to the symbol's file), only from `changed` today; propagated
    /// through union/intersect/minus (§4.5). Marks over a set draw range marks when present.
    pub ranges: BTreeMap<SymbolId, Vec<Range<usize>>>,
    pub deps: Deps,
    /// Non-fatal: unresolvable ids, unknown partition, git failure. Bubbled to ResolvedView.warnings.
    pub warnings: Vec<String>,
}
```

---

## 4. Resolution

### 4.0 `SetResolver`

```rust
// crates/outrider-view/src/set.rs
pub struct SetResolver<'a> {
    ctx: &'a ResolveCtx<'a>,                       // tree, index, layout, metrics, relations, partitions, session, repo_root
    sets: &'a BTreeMap<String, SetExpr>,           // ViewSpec.sets for Ref
    memo: &'a mut HashMap<u64, ResolvedSet>,       // owned by ViewResolver, keyed by structural hash; survives frames
    stack: Vec<String>,                            // Ref names being resolved (cycle detection)
    path_index: &'a PathIndex,                     // qualified_path → Vec<SymbolId>, built once per tree
    git: &'a mut GitCache,                         // §4.4
}
impl SetResolver<'_> {
    pub fn resolve(&mut self, expr: &SetExpr) -> ResolvedSet;      // recursive; memoised
    pub fn resolve_ref(&mut self, r: &SetRef) -> ResolvedSet;      // Name → sets[name] | Inline
    /// Deps an expression *would* have, without resolving (no ctx needed). Used by validate (01-space
    /// static-exclusion rule), by ViewResolver::needs_visible, and to decide memo reuse.
    pub fn static_deps(expr: &SetExpr, sets: &BTreeMap<String, SetExpr>) -> Deps;
    /// For 01-space plan_space (worker thread, no session): live leaves resolve empty + warning.
    pub fn static_only<'t>(tree: &'t SymbolTree, index: &'t TreeIndex, layout: Option<&'t PackLayout>, partitions: &'t PartitionRegistry, repo_root: &'t Path) -> SetResolver<'t>;
}
```

Algorithm for `resolve(expr)`:
1. `h = hash(expr)`; if `memo[h]` exists and `memo[h].deps ∩ dirty == ∅` (the `dirty` bits `ViewResolver::resolve` was called with) → return clone. (Memo entries whose deps intersect `dirty` are evicted at the start of `ViewResolver::resolve`; a `TREE` dirty clears the memo entirely.)
2. Match on the variant (§4.1–4.7); child expressions via recursive `resolve`; `deps` = union of children's deps ∪ the variant's own bits; `warnings` concatenated.
3. `Ref(name)`: if `name ∈ stack` → empty set + warning `"cycle"`; else push, resolve `sets[name]`, pop. Unknown name → empty + warning (validation should have rejected it).
4. Store in memo, return.

Result sizes are bounded by the tree; `HashSet<SymbolId>` clones are the main cost — `SymbolId` holds two `String`s, so prefer building results by iterating `index.iter()` once with a predicate where possible (glob/kind/fuzzy/where all do). Cache hits clone; acceptable (sets are small or rarely re-resolved).

### 4.1 `ids`: wire ids, bare paths, pseudo-ids

Each string is resolved in this order:
- `"$focus"` → `{session.focus}`, deps `FOCUS`. `"$hover"` → `session.hover` (empty if none), deps `HOVER`. `"$selection"` → `session.selection`, deps `SELECTION`. Unknown `$name` → warning.
- Wire form `"<kind>:<qualified_path>[#ordinal]"` (00-framework §2.7 `parse_wire`) → exact id if `index.node(&id).is_some()`, else warning `"id not found: …"` (parent §10 q1: tolerate silently, report).
- **Bare path** (no `kind:` prefix, e.g. `"src/auth/login.rs::verify"`, `"src/auth"`, `"src/auth/login.rs"`) → `path_index[path]`: *all* ids whose `qualified_path` equals the string (a `struct Point` and `impl Point` both match `src/lib.rs::Point`; a file and a same-named folder cannot collide because folders never end in `::…`). `#n` suffix selects an ordinal. Not found → warning. `PathIndex` = `HashMap<String, Vec<SymbolId>>` built by walking `index.iter()` once per tree (stored in `ViewResolver`, rebuilt on `TREE`).

Deps: union of the pseudo-id bits; static ids contribute `NONE` (a tree swap clears the memo anyway).

### 4.2 `glob`, `kind`, `fuzzy`

- `glob(p)`: `globset::GlobBuilder::new(p).literal_separator(true).build()?.compile_matcher()`; compiled matchers cached in `HashMap<String, GlobMatcher>` on the resolver. Match against `node.qualified_path` for every node except the root. `*` does not cross `/`; `::` and `#` are ordinary characters, so `src/**/*.rs` matches files, `src/**/*.rs::*` matches top-level items in them, `src/auth/**` matches everything under `src/auth` (files and items; the folder `src/auth` itself matches only `src/auth`). Invalid pattern → empty + warning. Descendants are **not** implied — use `descendants(glob(...))`; 01-space's exclusion closes over descendants itself.
- `kind(k | [k…])`: `k` compared to `SymbolKind::label()` (`"folder" | "file" | "chunk" | <item label>` — `fn`, `struct`, `impl`, `class`, `module`, `h1`, …). Aliases: `"item"` = any `Item`, `"leaf"` = `TreeIndex::is_leaf_item(node)`, `"container"` = `!leaf`. Root excluded.
- `fuzzy(q)`: `outrider_index::search::fuzzy_match(q, &node.name)` for every non-root, non-folder node (the palette's Symbol mode candidate rule, palette.rs:105–108; compose with `kind(file)` for File mode). Empty query matches everything (as the palette does). Decision on `fuzzy_match`'s home: **move to `outrider-index`** (`src/search.rs`, `pub use` from `lib.rs`); the app re-exports it from `palette.rs`. Rationale: `outrider-view` must not depend on the app crate, duplicating a 12-line function invites drift, and the palette becomes a Panel over `fuzzy` in milestone 5 anyway.

Deps: `NONE` (tree-only).

### 4.3 `where`, `reach`

- `where { metric, op, value }`: `let p = ctx.metrics.get(&metric)` (03-fill `MetricRegistry::get(&str) -> Option<&dyn MetricProvider>`); unknown → empty + warning. Domain = laid-out nodes (`ctx.layout.rects.keys()` → `index.node`) for which `p.value(node, &pctx)` is `Some`. Threshold: `Number(x)` → `x`; `Percentile("p90")` → nearest-rank percentile over the domain's values (sorted ascending, `idx = ceil(0.90 * n) - 1`, clamped) — computed once per `(metric, TREE)` and cached in the resolver as `percentile_tables: HashMap<String, Vec<f64>>` (03-fill defines the same table for scales; share it via `MetricRegistry::percentile_table(&metric, domain)` if 03-fill exposes one — prefer sharing). Membership: `cmp(value, op, threshold)`. Deps: `p.deps() ∪ METRICS ∪ TREE`.
- `reach { from, relation, direction, depth }`: `let r = ctx.relations.get(&relation)` (05-edges `RelationRegistry`); unknown → empty + warning. `seeds = resolve(from).ids`. BFS: frontier = seeds; for `d in 1..=depth` (∞ = `index.len()`): next = ∪ over frontier of `r.out_edges(id)` (Out), `r.in_edges(id)` (In), both (Both), targets only, minus `visited`; stop when `next` is empty. Result = `visited \ seeds` (seeds re-enter only if reached through a cycle — this makes `A = changed ∪ reach(changed, calls, in, 2)` (parent §4 diff-flow) read naturally). Deps: `from.deps ∪ r.deps() ∪ TREE`. Cost note: `calls` today re-parses per symbol (`resolve_calls`); the registry's per-symbol cache (05-edges) bounds it to one parse per visited symbol per session.

### 4.4 `changed(rev)` and the git probe

`crates/outrider-view/src/git.rs`:

```rust
pub enum ChangedSpec { Worktree, Rev(String), Range(String, String) }   // "worktree" | "HEAD~1" | "a..b"
pub fn parse_changed(s: &str) -> ChangedSpec;
/// One `git diff` (+ `git status --porcelain` for Worktree untracked files) → per-file new-side hunks.
pub fn changed_files(repo_root: &Path, spec: &ChangedSpec) -> Result<BTreeMap<String /*rel path, '/'*/, Vec<Range<usize> /*1-based line ranges, new side*/>>, String>;
pub struct GitCache { entries: HashMap<String /*spec text*/, (GitStamp, ResolvedSet)> }
```

- Commands (all via `outrider_index::churn::git_command(repo_root)`, `LC_ALL=C`): `Worktree` → `git diff -U0 --no-color --no-ext-diff HEAD` plus `git status --porcelain --untracked-files=all` (untracked `??` files → whole file, no ranges); `Rev(r)` → `git diff -U0 --no-color --no-ext-diff <r>` (r vs worktree, so `HEAD~1` = "everything since the last commit incl. uncommitted"); `Range(a,b)` → `git diff -U0 --no-color --no-ext-diff a b`. Parse `+++ b/<path>` for the file and `@@ -a[,b] +c[,d] @@` for new-side ranges `c..c+max(d,1)` (a pure-deletion hunk has `d=0` → mark the line `c` — the enclosing symbol still "changed"). Renames (`--find-renames` default) → new path. Non-zero exit / no git → `Err` → empty set + warning; no repo → same.
- Mapping to symbols: file path → `path_index[path]` File id (missing → the file is filtered out at index time; skip). For ranges: read `repo_root/path` bytes (`std::fs::read`), build `line_starts: Vec<usize>` (byte offset of each line start; O(bytes) once per changed file per resolve), convert each line range to a byte range `[line_starts[c-1], line_starts[min(c+d-1, n)] )`, then walk the file node's subtree: an item whose `byte_range` intersects the byte range is included; descend into it; keep the **deepest** intersecting items and also every ancestor item up to the file (so `descendants`/`children` behave; masks over `changed` light the file). `ranges[item] = byte_range ∩ item.byte_range` (clipped). Files without items (or `Chunk`s) get the file/chunk id + clipped ranges. Note the byte→line table is built from the *current* file bytes, which is what the diff's new side describes; if the file has changed since the tree was indexed, `byte_range`s are stale — tolerated (parent §10 q1).
- Deps: `GIT ∪ TREE`. `GitCache` entry key = spec text; validity stamp = `GitStamp { head: String, index_mtime: SystemTime, worktree_probe: u64 }` from the probe below; a `GIT` dirty bit evicts every entry (simple; `git diff` is ~tens of ms).

**Git probe** (pure, std-only, testable; also in `git.rs`):
```rust
pub struct GitProbe { git_dir: PathBuf, last: Option<GitStamp> }
impl GitProbe {
    /// Resolves `.git` (dir, or gitfile "gitdir: …" for worktrees/submodules). None if not a repo.
    pub fn open(repo_root: &Path) -> Option<GitProbe>;
    /// ~5 stats + reading .git/HEAD (and the ref file it names, else packed-refs mtime): returns
    /// Some(stamp) iff anything changed since the last call. Never spawns git.
    pub fn poll(&mut self) -> Option<GitStamp>;
    pub fn head_sha(&self) -> Option<String>;   // from the ref file / packed-refs; feeds SessionState.head
}
```
`GitStamp` compares: `.git/HEAD` content, mtime of the ref file HEAD points to (or `.git/packed-refs`), `.git/index` mtime+len (staging/commit), `.git/ORIG_HEAD`/`MERGE_HEAD` existence (rebase/merge in progress). Worktree edits that are not staged don't touch `.git`; the probe therefore also stats the *files currently in the last `changed` result* (`worktree_probe` = hash of their mtimes, capped at 256 files) so an editor save re-resolves `changed(worktree)` within a second. That is deliberately not a full-tree watcher.

### 4.5 Algebra, `children/ancestors/descendants/fileOf`, `not`

- `union` = ∪ ids, ranges merged per id (concatenate; no interval merge needed for painting). `intersect` = ∩ ids, ranges concatenated for surviving ids. `minus [a,b]` = `a.ids \ b.ids`, ranges from `a`. `not x` = `layout.rects.keys() \ x.ids` (the *laid-out* set, parent §2.2 — excluded/regrouped-away nodes are not "everything else"), no ranges, deps `x.deps ∪ TREE`.
- `children(s)` = ∪ `index.node(id).children` ids. `descendants(s)` = transitive children (exclusive of `s`). `ancestors(s)` = transitive `index.parent` (exclusive, includes root). `fileOf(s)` = for each id: itself if `kind == File`; else walk parents until a `File`; folders contribute nothing. Deps = `s.deps` (tree-only otherwise).
- Empty operand lists: `union []` = ∅; `intersect []` = ∅ (not "everything" — avoid a footgun); validated as a soft warning.

### 4.6 Session-live sets

- `neighbors(focus)` = `session.neighbors.iter().flatten()` (the app's cached `focus::neighbors` array, treemap.rs:1456); `None` → ∅. Deps `FOCUS ∪ TREE`.
- `visible(true)` = `session.visible` ids; when `session.visible` is `None` (first pass) → ∅ with deps `CAMERA` and the resolver records `pending_visible = true`. Deps `CAMERA ∪ TREE`. `visible(false)` is a validation error (there is no "invisible" set; use `not(visible)`).
- `$focus/$hover/$selection` — §4.1.

Two-pass flow (§5.2) guarantees a `visible()` set is never painted from the previous frame's camera.

### 4.7 Partitions: `community`, `layer`, and `regroup`

`crates/outrider-view/src/partition.rs`:

```rust
pub struct Partition {
    pub name: String,
    pub groups: BTreeMap<SymbolId, String>,      // member → group id/name (files or items; regroup uses files, 01-space §4.3)
    /// Group order for `layer` partitions (used by 05-edges `direction: up|down`); None = unordered communities.
    pub order: Option<Vec<String>>,
    pub basis: String,                            // inspectability, parent §1.3: "leiden on imports, 2026-08-14"
    pub deps: Deps,                               // METRICS for imported/derived, NONE for structural
}
impl Partition { pub fn group_of(&self, id: &SymbolId) -> Option<&str>; pub fn members(&self, group: &str) -> impl Iterator<Item=&SymbolId>; }

#[derive(Default)]
pub struct PartitionRegistry { parts: BTreeMap<String, Partition>, pub version: u64 }
impl PartitionRegistry {
    pub fn get(&self, name: &str) -> Option<&Partition>;
    pub fn insert(&mut self, p: Partition);      // bumps version (01-space SpaceKey.partition_version)
    /// Only built-in today: "topFolder" — every File mapped to its first path component ("" root files → "(root)").
    /// Structural (deps NONE); exists so regroup and community() are testable before real providers (parent §9 M8).
    pub fn builtin(tree: &SymbolTree) -> PartitionRegistry;
}
```

`community { partition, id }` and `layer { partition, name }` resolve identically: `partitions.get(partition)?.members(id)`. Two spellings exist for readability of documents (`layer` implies an ordered partition; if `order` is `None` a soft warning "partition has no layer order" is emitted). Unknown partition → ∅ + warning. Deps = `partition.deps ∪ TREE`. Real providers (Leiden communities over `imports`, an owner map from CODEOWNERS, an agent-imported partition) are milestone 8; each just calls `insert`.

### 4.8 Deps summary

| form | deps |
|---|---|
| `ids` static / `glob` / `kind` / `fuzzy` / `children` / `ancestors` / `descendants` / `fileOf` | `NONE` (memo cleared on TREE anyway) |
| `$focus` / `neighbors` | `FOCUS` |
| `$hover` | `HOVER` |
| `$selection` | `SELECTION` |
| `visible` | `CAMERA` |
| `changed` | `GIT` |
| `where` | provider deps ∪ `METRICS` |
| `reach` | from ∪ provider deps |
| `community` / `layer` | partition deps |
| `not` | operand ∪ `TREE` |
| `ref` | referenced set |

`static_deps` computes the same table without a context (unknown names → `NONE`).

### 4.9 Milestone-0 subset

For 00-framework §5 step 1, `set.rs` ships: `Ids` (all three id forms; `$focus`, `$hover`, `$selection`), `Neighbors`, `Union`, plus `Ref` (needed for `SetRef::Name`), the memo, `static_deps` for those variants, and `PathIndex`. Every other variant returns `∅` with warning `"<variant> not implemented yet"` and its table deps from §4.8 (so later milestones don't change invalidation semantics). This is exactly what `default_view` (00-framework §3.3) needs: `neighbors`, `focusSet = ids(["$focus"])`, `hoverSet = ids(["$hover"])`.

---

## 5. App integration

### 5.1 New files under `crates/outrider/src/view/`

- `git_watch.rs` — GPUI side of the probe:
  ```rust
  pub(crate) struct GitWatch { probe: Option<outrider_view::git::GitProbe>, pub head: Option<String> }
  impl GitWatch {
      pub fn new(repo_root: &Path) -> Self;
      /// Called from TreemapView::render (next to poll_loading, treemap.rs:4070). Cheap (a few stats).
      pub fn poll(&mut self) -> bool;             // true → caller sets view_dirty |= GIT
  }
  ```
  and a wake-up loop so a commit is noticed while the window is idle: `TreemapView::spawn_git_ticker(cx)` → `cx.spawn(async move |this, cx| loop { cx.background_executor().timer(Duration::from_secs(1)).await; if this.update(cx, |t, cx| { if t.git_watch.poll() { t.view_dirty |= Deps::GIT; t.git_head = t.git_watch.head.clone(); cx.notify(); true } else { false } }).is_err() { break } })`. Only spawn the ticker when the current `view_spec` has a `GIT`-dependent set (`ViewResolver::needs_git()` from `static_deps`); stop it when it no longer does. Poll interval 1 s (parent §7.7).
- No other new files; `session.rs` (00-framework §3.3) is unchanged.

### 5.2 `paint_items`: two-pass resolution around `visible_nodes`

Replace the single `resolve` call from 00-framework §3.2 with:

```rust
// pass 1 — before world::visible_nodes (treemap.rs:1483); visible: None
let session = SessionState { focus: &focus_id, hover: self.hover_id.as_ref(), selection: self.panel_selection(),
                             visible: None, head: self.git_watch.head.as_deref(), neighbors: Some(&neighbor_ids) };
let ctx = ResolveCtx { tree, index: &index, layout, metrics, relations, partitions, session, repo_root: &tree.repo_root };
let dirty = std::mem::take(&mut self.view_dirty);
self.view_resolver.resolve(&self.view_spec, &ctx, dirty);

let items = world::visible_nodes(tree, layout, &camera, vw, vh, |id| ...);   // unchanged (space)

// pass 2 — only if some set needs visible(); CAMERA-dependent artifacts recompute at most once per frame
if self.view_resolver.needs_visible() {
    let visible_ids: Vec<SymbolId> = items.iter().map(|it| it.node.id.clone()).collect();
    let ctx2 = ResolveCtx { session: SessionState { visible: Some(&visible_ids), ..session }, ..ctx };
    self.view_resolver.resolve_visible(&self.view_spec, &ctx2);   // re-resolves sets with CAMERA deps + layers over them
}
let resolved = self.view_resolver.current();                          // &ResolvedView
let ov = PaintOverrides { resolved };
for item in items { /* PaintItem construction as in 00-framework §3.2 */ }
```

`ViewResolver` additions: `needs_visible(&self) -> bool` (any set in the spec whose `static_deps` contains `CAMERA`), `resolve_visible(&mut self, spec, ctx)` (equivalent to `resolve(spec, ctx, Deps::CAMERA)` but with `session.visible` populated), `current(&self) -> &ResolvedView`. Borrow note: `items` borrows `self.tree`; `visible_ids` is an owned clone precisely so pass 2 can take `&mut self.view_resolver` — same split-borrow pattern 00-framework §3.2 prescribes (destructure `self` fields into locals first). Cost when no `visible()` set exists: one `bool` check.

### 5.3 Other wiring

- `render` (treemap.rs:4069–4070): `needs_notify |= self.git_watch.poll().then(|| { self.view_dirty |= Deps::GIT; true }).unwrap_or(false);` — belt and braces with the ticker (a frame that is already rendering picks the change up immediately).
- `install_project_preview` (treemap.rs:2910): `self.git_watch = GitWatch::new(&project_root)`; `self.partitions = PartitionRegistry::builtin(&tree)` (01-space §5.1); rebuild `PathIndex` (`view_resolver.invalidate_all()` does it lazily on next resolve).
- `TreemapView` fields: `git_watch: view::git_watch::GitWatch`, `partitions: PartitionRegistry` (shared with 01-space), and `git_head` may simply be `git_watch.head` (drop the separate field from 00-framework §3.2's snippet).
- `SessionState.neighbors`: pass the array already computed at treemap.rs:1456; no second `focus::neighbors` call.
- Palette (`palette.rs`) is untouched until milestone 5; only `fuzzy_match` moves.

---

## 6. Commands

- `ViewCommand::DefineSet { name, expr }` (00-framework §2.5) — replaces by name; `Applied.changed = SPEC`; the memo entry for the *old* expression simply stops being referenced.
- `Apply` / `Patch` with `sets` (merge by name), `Clear(Sets)` (removes all named sets; layers referencing them then fail validation → `apply` refuses, so `Clear(Sets)` must be applied together with `Clear(Layers)` or by `Clear(All)` — document in 11-cli).
- CLI (parent §6.2): `outrider set <name> --ids a,b --glob p --kind k --fuzzy q --changed rev --reach <relation>:<in|out|both>:<n|inf> --where <metric><op><value> --union … --minus …` compiles to one `SetExpr` (flags combine as `intersect` unless `--union` is given; 11-cli specifies the exact grammar). `outrider query set <name> [--json] [--missing]` prints resolved members (and `warnings`), using the RPC `query.set` method against the running resolver — the same `ResolvedSet` the paint uses.
- Offline `query set` (CLI without a running app) uses `SetResolver::static_only` over a freshly loaded index; live forms report `"live set: requires a running instance"`.

---

## 7. Invalidation

| Deps bit | set by | drops |
|---|---|---|
| `TREE` | `PackingGeometryState::apply_snapshot/finish`, loader completion (00-framework §3.4) | whole memo, `PathIndex`, percentile tables, `GitCache` |
| `FOCUS` | `Focus::set/step_*`, arrow moves; focus reconcile after repack (01-space §5.4) | `$focus`, `neighbors`, and everything built on them |
| `HOVER` | `on_mouse_move` when `hover_id` changes | `$hover` |
| `SELECTION` | panel row move (08-panel) | `$selection` |
| `CAMERA` | pan/zoom/tween | `visible()`; re-resolved in pass 2 (§5.2) |
| `GIT` | `GitWatch::poll` (ticker or render) | `changed(...)` + `GitCache`; also churn/cochange (03/05) |
| `METRICS` | imported metric (re)load; `PartitionRegistry::insert` | `where`, `community/layer` |
| `SPEC` | `apply_view_command` | nothing in the memo directly; `ViewResolver` re-walks the spec and reuses memo hits |

---

## 8. Migration steps

1. **Milestone 0** (with 00-framework §5): `SetExpr` full enum + serde round-trip tests for every form (parent §5 example file); `ResolvedSet`; `SetResolver` with `Ref/Ids/Neighbors/Union`, pseudo-ids, wire ids, bare paths, `PathIndex`, memo, cycle detection, `static_deps`; stubs for the rest (§4.9). Move `TreeIndex` (00-framework §2.8). No app behaviour change: `default_view` resolves to today's focus/neighbor/hover sets.
2. Move `fuzzy_match` to `outrider-index/src/search.rs`; `palette.rs` re-exports; run palette tests unchanged.
3. `glob`, `kind`, `fuzzy`, `children/ancestors/descendants/fileOf`, `intersect/minus/not` (milestone 2 needs `glob` for the walking skeleton). Add `globset` dep (already in the framework `Cargo.toml`).
4. `partition.rs` with `builtin("topFolder")`; `community/layer`; `ResolveCtx.partitions` (framework delta 1). Enables 01-space regroup tests.
5. `git.rs`: `parse_changed`, `changed_files`, hunk→symbol mapping, `GitCache`, `GitProbe`; make `churn::git_command/git_stdout` public. App: `view/git_watch.rs`, ticker, `render` poll, `git_head` from the probe. Golden: no behaviour change when no `changed` set exists (ticker not spawned).
6. `visible()` two-pass (§5.2) — `needs_visible/resolve_visible/current` on `ViewResolver`. Golden: with no `visible()` set, `paint_items` runs pass 1 only (assert via a counter in tests of `ViewResolver`).
7. `where` (after 03-fill's `MetricRegistry` exists) and `reach` (after 05-edges' `RelationRegistry`), milestone 6.
8. Nothing is deleted by this spec. `TreemapView.neighbors`, `hover_id`, palette code remain; they are the *sources* live sets read from.

---

## 9. Tests

`crates/outrider-view/tests/set_test.rs` (fixture: `common::copy_fixture("mini_repo")` copied from `outrider-index/tests/common/mod.rs`; tree via `outrider_index::index_repo(dir, &[], &[])`; layout via `outrider_layout::pack` with the `world::pack_config` constants; a `ctx()` helper building `ResolveCtx` with empty registries and a `SessionState` whose focus is `src/lib.rs::free`):

- `ids_wire_bare_and_pseudo`: `["fn:src/lib.rs::free", "src/lib.rs::Point", "$focus", "nope:src/x.rs"]` → `{free, struct Point, impl Point, focus}` + one warning; deps `FOCUS`.
- `ids_bare_path_ordinal`: `"src/lib.rs::Point#1"` → impl only.
- `glob_matches_paths_not_descendants`: `src/**` → files+items under src, not folder `src`; `src/*.rs` → the two files only; `**/*_test.rs` → ∅.
- `kind_labels_and_aliases`: `kind("fn")` = `{helper, new, norm, free, clamp}`; `kind("leaf")` ⊇ those; `kind(["file","folder"])`.
- `fuzzy_like_palette`: `fuzzy("hlp")` = `{helper}`; equals `palette::fuzzy_match` results on the same names (test lives in the app crate to compare).
- `algebra_laws`: `union(a,b) == union(b,a)`; `intersect(a, not(a)) == ∅`; `minus(a,a) == ∅`; `not(∅) == layout.rects.keys()`; `not` excludes ids that have no rect (prune one file with 01-space `prune_tree`, re-pack, assert).
- `structural_navigation`: `children(ids[src/lib.rs])` = the four top-level items; `descendants` adds `new`,`norm`; `ancestors(ids[norm])` = `{impl Point, src/lib.rs, src, root}`; `fileOf(ids[norm, src])` = `{src/lib.rs}`.
- `ref_cycle_and_unknown`: `sets = {a: ref b, b: ref a}` → ∅ + "cycle" warning; `ref zzz` → warning.
- `memo_reuse_and_eviction`: resolve `neighbors` twice with `dirty = NONE` → provider called once; `dirty = FOCUS` → recomputed; `dirty = TREE` → memo empty.
- `static_deps_table`: one assertion per row of §4.8; `static_deps(ref hidden)` follows the reference; cycles terminate.
- `neighbors_from_session`: session array `[Some(a), None, Some(b), None]` → `{a, b}`.
- `visible_two_pass`: `visible` with `session.visible = None` → ∅, deps CAMERA, `needs_visible() == true`; with `Some(&[ids])` → those ids.
- `partition_topfolder_and_community`: `builtin(tree)["topFolder"]` maps `src/lib.rs`, `src/util.rs` → `"src"`, `generated/junk.rs` → `"generated"`, `README.md` → `"(root)"`; `community{topFolder, "src"}` = the two files; unknown partition → warning.
- `changed_on_temp_repo` (`git_fixture()` pattern from `churn_test.rs`, then modify `src/util.rs` line 2 without committing): `changed("worktree")` = `{src/util.rs, fn clamp}` with `ranges[clamp]` non-empty and inside `clamp.byte_range`; `changed("HEAD~1")` after committing that edit = same; `changed("HEAD~2..HEAD~1")` = `{src/lib.rs}` (the fixture's `// x` commit); untracked new file → File id, no ranges; not-a-repo temp dir → ∅ + warning.
- `git_probe_detects_commit_and_index`: `probe.poll()` → `Some` initially, `None` idle, `Some` after `git commit`, `Some` after `git add`, `head_sha()` equals `git rev-parse HEAD`.
- `where_percentile_and_ops` (after 03-fill): a fake `MetricProvider` returning `measure`; `where(measure > p50)` = top half by nearest rank; `== 3.0`; unknown metric → warning.
- `reach_bfs_depth_and_direction` (after 05-edges): a fake `RelationProvider` over a 5-node chain with a back edge; depth 1/2/inf; `In` vs `Out`; seeds excluded unless cycled.

App crate: `view/git_watch.rs` — `poll_returns_false_when_no_repo`; `treemap.rs` tests — `paint_decisions` golden from 00-framework §4 still passes; `palette` tests unchanged after the `fuzzy_match` move.

Manual: `.outrider/views/changed.json` with `sets.c = {changed:"worktree"}` and a `mask dimExcept c` (04-mask); edit a file → within ~1 s the box lights; commit → goes dark; `git checkout HEAD~1` → boxes follow.

---

## 10. Open questions / risks

1. **`git diff` on the UI thread.** First resolve after a GIT tick blocks for the diff (typically < 50 ms; large rebases more). If it shows, move `changed_files` into the ticker task (background executor) and hand the result to the resolver via `GitCache::prefill`.
2. **`SymbolId` clone cost** in big sets (two `String`s per id). If `where(...)` over 100k nodes hurts, intern ids (`Arc<SymbolId>` or a dense `u32` index in `TreeIndex`) — a framework-wide change; not needed for the walking skeleton.
3. **Bare-path ambiguity** (`src/lib.rs::Point` → struct + impl) is by design; agents that need one use the wire form. Should `ids` warn on multi-match? Currently no.
4. **`changed(rev)` semantics** (rev vs worktree, not rev vs HEAD) matches `git diff <rev>`; if agents expect "the commit rev", they write `rev~1..rev`. Document in CLI help.
5. **Untracked files in `worktree`** need `git status`; on huge repos that is slower than the diff. Cap or make optional (`changed("worktree:tracked")`)?
6. **`intersect []` = ∅** vs "universe": chose ∅ (safer for masks); revisit if documents want `intersect` as a filter chain starting from everything (they can start with `not(∅)`… or better `{"kind":"leaf"}`).
7. **Percentile tables** are per metric over the laid-out domain; a `Fill` with a `domain` (03-fill) uses a different table. `where` intentionally always uses the whole laid-out set (documented).

---

## 11. Framework deltas

1. **`ResolveCtx.partitions: &'a PartitionRegistry`** (00-framework §2.4) — shared with 01-space delta 1.
2. **`ViewResolver` API** (00-framework §2.4): add `needs_visible(&self) -> bool`, `needs_git(&self) -> bool`, `resolve_visible(&mut self, spec, ctx)`, `current(&self) -> &ResolvedView`; and state it owns the set memo (`HashMap<u64, ResolvedSet>`), `PathIndex`, `GitCache`, percentile tables — all cleared by `invalidate_all()`/`TREE`.
3. **`SessionState.head`** is sourced from `GitWatch.head` (probe), not a separate `git_head` field (00-framework §3.2 snippet); `SessionState.neighbors` is the array from `treemap.rs:1456`.
4. **Module layout (00-framework §2.1):** add `partition.rs`, `git.rs`; app `view/` gains `git_watch.rs` (00-framework §3 lists `watch.rs` for the *view-file* watcher — keep both, distinct names).
5. **`outrider-index` additions** (not in 00-framework): `search.rs` (`fuzzy_match`), `pub` `churn::git_command/git_stdout`. `outrider-view/Cargo.toml` needs no new deps beyond `globset` (git via `std::process::Command`).
6. **`validate()` signature** unchanged; partition existence is a resolve-time warning, not a validation input.
7. **00-framework §3.4 table**: the "git watcher tick → GIT" row is implemented by `GitWatch::poll` (ticker + `render`), as specified here.
