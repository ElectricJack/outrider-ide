# Reliability, Cache, and Indexing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Outrider open immediately, index without redundant file reads, prioritize visible texture work, enforce global memory and per-project disk-cache limits, avoid repository mutations, and pass cross-platform quality gates.

**Architecture:** Keep the existing index/layout/render layers, but move project loading into a generation-guarded background service and make texture generation an on-demand priority queue. Cache identity includes project, source, render schema, and theme inputs; disk storage is namespaced and bounded per project in the OS cache directory. Split `TreemapView` helpers along behavior boundaries after the new services are covered by tests.

**Tech Stack:** Rust 2021, GPUI, Rayon, tree-sitter, serde/serde_json, image, tempfile, GitHub Actions.

## Global Constraints

- Global memory-cache allowance is stored in MB.
- Per-project disk-cache allowance is stored outside repositories and defaults to exactly 1 GiB (`1_073_741_824` bytes).
- Internal symbol paths use `/`; filesystem access uses `Path` and `PathBuf`.
- Opening a repository must not create or modify files inside it.
- A project becomes interactive after indexing and layout, without waiting for texture pre-baking.
- Existing treemap layout, navigation controls, syntax colors, and supported languages remain unchanged.
- Every behavior change follows red-green-refactor and keeps the focused test target green before proceeding.

---

### Task 1: Restore a Portable, Warning-Clean Baseline

**Files:**
- Modify: `crates/outrider-index/tests/scan_test.rs`
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/src/parse.rs`
- Modify: all Rust sources changed by `cargo fmt`

**Interfaces:**
- Consumes: existing `scan_files`, `IndexProgress`, and parser callback behavior.
- Produces: portable path assertions, `Default for IndexProgress`, and named parser callback aliases.

- [ ] **Step 1: Make the path regression test platform-neutral**

Replace the string conversion in `scan_respects_gitignore_and_builds_sorted_tree` with component vectors:

```rust
let paths: Vec<Vec<String>> = files
    .iter()
    .map(|file| {
        file.rel_path
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect()
    })
    .collect();
assert_eq!(
    paths,
    vec![
        vec!["README.md"],
        vec!["src", "lib.rs"],
        vec!["src", "util.rs"],
    ]
);
```

- [ ] **Step 2: Verify the Windows regression is green**

Run: `cargo test -p outrider-index --test scan_test`

Expected: `1 passed; 0 failed`.

- [ ] **Step 3: Resolve the two current Clippy failures**

Add:

```rust
impl Default for IndexProgress {
    fn default() -> Self {
        Self::new()
    }
}
```

Define and use:

```rust
type KindClassifier = dyn for<'tree> Fn(&str, Node<'tree>, &[u8]) -> Option<&'static str>;
type NameExtractor = dyn for<'tree> Fn(Node<'tree>, &[u8]) -> String;
```

- [ ] **Step 4: Format and verify the baseline**

Run: `cargo fmt --all`

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Run: `cargo test --workspace`

Expected: all commands exit successfully.

- [ ] **Step 5: Commit the baseline**

```text
git add crates
git commit -m "chore: restore portable warning-clean baseline"
```

### Task 2: Add Validated Global and Per-Project Cache Settings

**Files:**
- Modify: `crates/outrider/src/settings.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: canonical project paths and existing JSON settings.
- Produces: `Settings::disk_cache_bytes(&Path) -> u64`, `Settings::set_disk_cache_bytes(&Path, u64)`, and recoverable load/save results.

- [ ] **Step 1: Write settings tests first**

Add tests using a pure project-key helper and JSON round trips:

```rust
#[test]
fn new_project_defaults_to_one_gibibyte() {
    let settings = Settings::default();
    assert_eq!(settings.disk_cache_bytes_for_key("D:/repo"), 1_073_741_824);
}

#[test]
fn project_disk_limits_are_independent() {
    let mut settings = Settings::default();
    settings.set_disk_cache_bytes_for_key("D:/one".into(), 512 * 1024 * 1024);
    settings.set_disk_cache_bytes_for_key("D:/two".into(), 2 * 1024 * 1024 * 1024);
    assert_eq!(settings.disk_cache_bytes_for_key("D:/one"), 512 * 1024 * 1024);
    assert_eq!(settings.disk_cache_bytes_for_key("D:/two"), 2 * 1024 * 1024 * 1024);
}

#[test]
fn old_settings_json_receives_disk_defaults() {
    let settings: Settings = serde_json::from_str(
        r#"{"filter_extensions":[],"filter_folders":[],"show_welcome":false,"cache_mb":128}"#,
    )
    .unwrap();
    assert_eq!(settings.disk_cache_bytes_for_key("repo"), 1_073_741_824);
}
```

- [ ] **Step 2: Run tests and confirm the missing API fails**

Run: `cargo test -p outrider settings::tests`

Expected: compilation fails because the disk-limit map and helper methods do not exist.

- [ ] **Step 3: Implement the settings model**

Add a `BTreeMap<String, u64>` with `#[serde(default)]`, a `DEFAULT_DISK_CACHE_BYTES` constant, canonical project-key normalization, and range validation. Change persistence to:

```rust
pub enum SettingsLoad {
    Loaded(Settings),
    Recovered { settings: Settings, warning: String },
}

pub fn load() -> SettingsLoad;
pub fn save(&self) -> Result<(), String>;
```

On malformed JSON, rename the file to `settings.invalid.json` before returning defaults. Use a temporary sibling file plus `rename` for atomic settings writes.

- [ ] **Step 4: Integrate the current-project disk field**

Extend `SettingsDraft` with `disk_cache_gb: String`. Parse decimal GiB into a bounded `u64`, save it for `tree.repo_root`, and retain the existing global memory MB field. Surface validation and save errors through a notification field rather than silently substituting values.

- [ ] **Step 5: Verify and commit settings behavior**

Run: `cargo test -p outrider settings::tests`

Run: `cargo clippy -p outrider --all-targets -- -D warnings`

```text
git add crates/outrider/src/settings.rs crates/outrider/src/treemap.rs
git commit -m "feat: add per-project disk cache settings"
```

### Task 3: Build a Project-Scoped, Validated, Bounded Disk Texture Store

**Files:**
- Create: `crates/outrider/src/texture_store.rs`
- Modify: `crates/outrider/src/main.rs`
- Modify: `crates/outrider/src/rasterize.rs`
- Modify: `crates/outrider/src/theme.rs`

**Interfaces:**
- Consumes: project root, source fingerprint, `SymbolId`, renderer schema, theme fingerprint, and disk byte limit.
- Produces: `TextureStore::open`, `TextureStore::load`, `TextureStore::save`, `TextureStore::clear`, `TextureStore::used_bytes`, and per-project LRU eviction.

- [ ] **Step 1: Write failing store tests**

Cover isolation, invalidation, corruption, and limits with real temporary directories:

```rust
#[test]
fn identical_symbols_in_different_projects_do_not_share_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut one = TextureStore::open_at(dir.path(), "project-one", 1024).unwrap();
    let two = TextureStore::open_at(dir.path(), "project-two", 1024).unwrap();
    one.save(&key("src/lib.rs", 11), &payload(16)).unwrap();
    assert!(two.load(&key("src/lib.rs", 11)).unwrap().is_none());
}

#[test]
fn source_fingerprint_changes_the_cache_key() {
    assert_ne!(key("src/lib.rs", 11), key("src/lib.rs", 12));
}

#[test]
fn corrupt_length_is_rejected_without_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = TextureStore::open_at(dir.path(), "project", 1024).unwrap();
    store.write_raw_for_test(&key("a.rs", 1), &[0xff; 12]).unwrap();
    assert!(store.load(&key("a.rs", 1)).unwrap().is_none());
}

#[test]
fn saving_past_limit_evicts_oldest_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = TextureStore::open_at(dir.path(), "project", 40).unwrap();
    store.save(&key("old.rs", 1), &payload(24)).unwrap();
    store.save(&key("new.rs", 1), &payload(24)).unwrap();
    assert!(store.load(&key("old.rs", 1)).unwrap().is_none());
    assert!(store.load(&key("new.rs", 1)).unwrap().is_some());
    assert!(store.used_bytes() <= 40);
}
```

- [ ] **Step 2: Verify the store tests fail because the module is absent**

Run: `cargo test -p outrider texture_store::tests`

Expected: compilation fails because `texture_store` and its APIs do not exist.

- [ ] **Step 3: Implement keying and file validation**

Create `TextureKey` from stable string/byte inputs using a deterministic in-repository FNV-1a helper. Use a header containing magic `OUTRTX01`, width, height, payload length, and last-access timestamp. Reject dimensions above the renderer caps, reject `len != width * height * 4`, and reject files larger than the configured project allowance before allocating.

Write to `<key>.tmp`, flush, and rename to `<key>.tex`. Maintain an in-memory metadata index built from validated headers at store open. Touch access metadata on successful loads.

- [ ] **Step 4: Integrate the store with textures**

Replace the global `dirs::cache_dir()/outrider/textures` directory and `DefaultHasher` key with a `TextureStore` owned by `TextureCache`. Include a `RENDER_SCHEMA_VERSION` constant and `theme::fingerprint()` in keys. Derive source fingerprints while indexing and retain them on file nodes or in a path map passed to the cache.

- [ ] **Step 5: Verify limits and commit**

Run: `cargo test -p outrider texture_store::tests rasterize::tests`

Run: `cargo clippy -p outrider --all-targets -- -D warnings`

```text
git add crates/outrider/src
git commit -m "feat: add bounded project texture store"
```

### Task 4: Enforce Memory Accounting and Viewport-Priority Scheduling

**Files:**
- Modify: `crates/outrider/src/rasterize.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: visible `SymbolId` requests with screen-area priority.
- Produces: deduplicated `TextureCache::request`, bounded `TextureCache::process_requests`, correct replacement accounting, and direct-child compositing access.

- [ ] **Step 1: Add failing cache behavior tests**

```rust
#[test]
fn repeated_request_is_deduplicated_and_priority_is_upgraded() {
    let mut cache = memory_cache(1024);
    cache.request(sid("a"), 10.0);
    cache.request(sid("a"), 100.0);
    cache.request(sid("b"), 50.0);
    assert_eq!(cache.queued_ids(), vec![sid("a"), sid("b")]);
}

#[test]
fn replacing_an_entry_does_not_double_count_bytes() {
    let mut cache = memory_cache(1024);
    cache.insert(sid("a"), texture(100));
    cache.insert(sid("a"), texture(60));
    assert_eq!(cache.used_bytes(), 60);
}

#[test]
fn disk_promotion_obeys_memory_limit() {
    let mut cache = memory_cache(100);
    cache.insert(sid("a"), texture(80));
    cache.insert(sid("b"), texture(80));
    assert!(cache.used_bytes() <= 100);
}
```

- [ ] **Step 2: Run and confirm the regressions fail**

Run: `cargo test -p outrider rasterize::tests`

Expected: missing request API and incorrect replacement/eviction assertions fail.

- [ ] **Step 3: Implement a deduplicated priority queue**

Use `HashMap<SymbolId, f64>` for pending requests. `request` retains the maximum priority. `process_requests` sorts only the drained unique requests, bakes at most `BAKES_PER_FRAME`, reinserts unprocessed requests, and reports whether work remains.

Centralize insertion in one method that subtracts replaced bytes, retires replaced images, adds new bytes, and calls eviction. Call it for bakes, disk promotions, and explicit insertions.

- [ ] **Step 4: Remove whole-cache snapshots and full pre-baking**

Delete `pre_bake_all`. Replace `child_bytes_snapshot` with a method that clones image bytes only for the requested node's direct children. In `paint_items`, request visible textures using screen area and request only dependencies needed for the visible container being baked.

- [ ] **Step 5: Verify and commit scheduling**

Run: `cargo test -p outrider rasterize::tests world::tests treemap::tests`

```text
git add crates/outrider/src/rasterize.rs crates/outrider/src/treemap.rs
git commit -m "feat: prioritize visible textures within cache limits"
```

### Task 5: Stop Mutating Repositories for Churn Metadata

**Files:**
- Modify: `crates/outrider-index/src/churn.rs`
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/Cargo.toml`
- Modify: `crates/outrider-index/tests/churn_test.rs`

**Interfaces:**
- Consumes: repository root and optional application cache root.
- Produces: `ChurnOutcome { counts, warning }` where cache/Git failures are non-fatal.

- [ ] **Step 1: Write repository-integrity and degraded-cache tests**

```rust
#[test]
fn churn_cache_is_not_written_inside_repository() {
    let repo = git_repo();
    let cache = tempfile::tempdir().unwrap();
    churn_counts_with_cache(repo.path(), cache.path()).unwrap();
    assert!(!repo.path().join(".outrider").exists());
}

#[test]
fn unwritable_cache_still_returns_counts() {
    let repo = git_repo();
    let invalid_cache_root = repo.path().join("regular-file");
    std::fs::write(&invalid_cache_root, b"x").unwrap();
    let outcome = churn_counts_with_cache(repo.path(), &invalid_cache_root).unwrap();
    assert!(!outcome.counts.is_empty());
    assert!(outcome.warning.is_some());
}
```

- [ ] **Step 2: Verify the old repository-local behavior fails the tests**

Run: `cargo test -p outrider-index --test churn_test`

Expected: the repository-integrity assertion fails or the new API is missing.

- [ ] **Step 3: Implement OS-cache namespacing and graceful degradation**

Add `dirs = "6"` to `outrider-index`. Hash the canonical repository path into `dirs::cache_dir()/outrider/churn/<project>/churn-cache.json`. Treat Git absence/non-repository as empty counts without warning; treat cache read/write failures as warnings while retaining computed counts.

Thread warnings through the indexing result so the UI can display them after a successful load.

- [ ] **Step 4: Verify and commit churn behavior**

Run: `cargo test -p outrider-index churn -- --nocapture`

Run: `cargo test -p outrider-index --test churn_test`

```text
git add crates/outrider-index
git commit -m "fix: keep churn cache outside analyzed repositories"
```

### Task 6: Consolidate Indexing Around One Supported-File Read

**Files:**
- Modify: `crates/outrider-index/src/scan.rs`
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/src/types.rs`
- Modify: `crates/outrider-index/tests/index_test.rs`

**Interfaces:**
- Consumes: discovered paths and filter settings.
- Produces: `IndexedFile` records containing metrics, source fingerprint, parsed items, and optional chunks from one owned buffer.

- [ ] **Step 1: Write tests for one-pass file products and size safeguards**

Introduce a test-only counting reader through a small `FileSource` trait:

```rust
#[test]
fn supported_file_is_opened_once_for_metrics_parse_and_chunks() {
    let source = CountingSource::with_file("src/lib.rs", b"fn one() {}\n");
    let indexed = index_discovered_files(&source, &[PathBuf::from("src/lib.rs")], None).unwrap();
    assert_eq!(source.opens("src/lib.rs"), 1);
    assert_eq!(indexed[0].lines, 1);
    assert_eq!(indexed[0].parsed.items.len(), 1);
}

#[test]
fn oversized_unsupported_file_is_stream_counted_without_full_read() {
    let source = CountingSource::with_repeated_file("data", b'x', MAX_RETAINED_FILE_BYTES + 1);
    let indexed = index_discovered_files(&source, &[PathBuf::from("data")], None).unwrap();
    assert_eq!(source.full_reads("data"), 0);
    assert_eq!(indexed[0].bytes, (MAX_RETAINED_FILE_BYTES + 1) as u64);
}
```

- [ ] **Step 2: Verify the wished-for indexing API fails to compile**

Run: `cargo test -p outrider-index index::tests`

Expected: compilation fails because `FileSource`, `IndexedFile`, and the single-pass function do not exist.

- [ ] **Step 3: Implement discovery and materialization phases**

Make scanning return paths and inexpensive metadata only. For supported parsers and retained text files, read once and compute bytes, lines, fingerprint, parse output, file docs, and chunks. For files beyond `MAX_RETAINED_FILE_BYTES` or unsupported formats, use `BufRead::read_until` to count lines without retaining the entire body.

Build the tree directly from `IndexedFile` values rather than rereading files in `build_tree`. Preserve deterministic sorting and normalized qualified paths.

- [ ] **Step 4: Verify parsing, scanning, and dump behavior**

Run: `cargo test -p outrider-index`

Run: `cargo test -p outrider-layout`

- [ ] **Step 5: Commit the indexing pipeline**

```text
git add crates/outrider-index
git commit -m "perf: index supported files in one read"
```

### Task 7: Open Immediately and Guard Background Project Loads

**Files:**
- Create: `crates/outrider/src/project_loader.rs`
- Modify: `crates/outrider/src/main.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: project path, settings snapshot, and monotonically increasing load generation.
- Produces: `ProjectLoader::start`, `ProjectLoader::poll`, `LoadProgress`, and generation-checked `LoadResult` without texture pre-baking.

- [ ] **Step 1: Write generation and stale-result tests**

```rust
#[test]
fn newer_load_supersedes_older_result() {
    let mut loader = ProjectLoader::new();
    let first = loader.begin_generation();
    let second = loader.begin_generation();
    assert!(!loader.accepts(first));
    assert!(loader.accepts(second));
}

#[test]
fn successful_load_contains_tree_and_layout_without_textures() {
    let result = load_project(fixture_path(), &Settings::default(), progress()).unwrap();
    assert!(!result.tree.root.children.is_empty());
    assert!(!result.layout.rects.is_empty());
}
```

- [ ] **Step 2: Run tests and confirm the loader API is missing**

Run: `cargo test -p outrider project_loader::tests`

Expected: compilation fails because `project_loader` is not defined.

- [ ] **Step 3: Extract and implement the loader**

Move `LoadedProject`, `LoadingState`, thread spawning, progress polling, and stale-generation checks into `project_loader.rs`. The load result contains tree, layout, warnings, and source fingerprints; it does not contain a populated texture cache.

Change `TreemapView` to support a loading-shell constructor without a tree. On success, install the project and create an empty project-scoped `TextureCache`. On error, retain the previous project and add a notification.

- [ ] **Step 4: Change initial startup ordering**

In `main`, resolve the requested folder and load settings, then call `application().run` immediately. Construct the loading shell inside `open_window` and start the background load from the view constructor. Remove synchronous `index_repo` and `pack` calls from `main`.

- [ ] **Step 5: Verify and commit asynchronous loading**

Run: `cargo test -p outrider project_loader::tests treemap::tests`

Run: `cargo check -p outrider`

```text
git add crates/outrider/src/main.rs crates/outrider/src/project_loader.rs crates/outrider/src/treemap.rs
git commit -m "feat: open UI before background project indexing"
```

### Task 8: Add Notifications and Split Treemap Responsibilities

**Files:**
- Create: `crates/outrider/src/navigation.rs`
- Create: `crates/outrider/src/interaction.rs`
- Create: `crates/outrider/src/overlays.rs`
- Create: `crates/outrider/src/paint_model.rs`
- Modify: `crates/outrider/src/main.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: existing `TreemapView` state and approved behavior boundaries.
- Produces: `NavigationHistory`, `Notification`, overlay builders, interaction actions, and paint-model helpers with unchanged rendered behavior.

- [ ] **Step 1: Add pure behavior tests before extraction**

Move the existing history tests to target:

```rust
let mut history = NavigationHistory::new(root.clone(), 64);
history.push(child.clone());
assert_eq!(history.back(), Some(&root));
assert_eq!(history.forward(), Some(&child));
```

Add notification tests:

```rust
#[test]
fn newest_notification_is_visible_and_dismissible() {
    let mut notifications = Notifications::default();
    notifications.push(Notification::warning("cache unavailable"));
    assert_eq!(notifications.visible().unwrap().message, "cache unavailable");
    notifications.dismiss_visible();
    assert!(notifications.visible().is_none());
}
```

- [ ] **Step 2: Run the focused tests and confirm new types are absent**

Run: `cargo test -p outrider navigation::tests overlays::tests`

Expected: compilation fails because the extracted types do not exist.

- [ ] **Step 3: Extract pure navigation and notification state**

Move navigation history mechanics into `navigation.rs`. Move notification state and UI builders into `overlays.rs`. Keep GPUI callbacks in `TreemapView` when they require direct mutable access, but have them emit or consume small action enums from `interaction.rs`.

- [ ] **Step 4: Extract paint preparation and remaining overlays**

Move paint data structures and pure formatting/geometry helpers into `paint_model.rs`. Move welcome, settings, context-menu, loading, and notification element construction into `overlays.rs`. Keep `TreemapView::render` as the composition point and preserve all existing event listeners and visual constants.

- [ ] **Step 5: Verify behavior-preserving refactoring and commit**

Run: `cargo test -p outrider`

Run: `cargo clippy -p outrider --all-targets -- -D warnings`

```text
git add crates/outrider/src
git commit -m "refactor: split treemap view responsibilities"
```

### Task 9: Add CI, License, Documentation, and Final Verification

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `LICENSE`
- Modify: `README.md`
- Modify: `Cargo.toml`
- Modify: workspace Rust files through formatting

**Interfaces:**
- Consumes: completed workspace behavior.
- Produces: three-platform CI, correct licensing, documented cache behavior, and a clean verification baseline.

- [ ] **Step 1: Add the three-platform workflow**

Create a matrix over `ubuntu-latest`, `windows-latest`, and `macos-latest`. Install stable Rust with `rustfmt` and `clippy`, then run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On Linux, install the GPUI system packages required by the pinned Zed revision before compilation:

```text
sudo apt-get update
sudo apt-get install -y build-essential clang pkg-config libfontconfig-dev libasound2-dev libssl-dev libzstd-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev libvulkan1 mesa-vulkan-drivers
```

- [ ] **Step 2: Add release metadata**

Add the standard MIT license text with copyright year 2026. Add `license = "MIT"`, `repository`, and `rust-version = "1.80"` where appropriate in workspace package metadata without changing dependency versions.

- [ ] **Step 3: Document cache and repository behavior**

Update README settings documentation to state:

- Memory limit is global.
- Disk limit is per project and defaults to 1 GB.
- Caches live in the operating system cache directory.
- Visible nodes are prioritized.
- Outrider does not write cache files into analyzed repositories.

- [ ] **Step 4: Run fresh full verification**

Run: `cargo fmt --all`

Run: `cargo fmt --all -- --check`

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Run: `cargo test --workspace`

Run: `git diff --check`

Expected: every command exits with status 0 and the workspace test output reports no failures.

- [ ] **Step 5: Commit release-quality files**

```text
git add .github LICENSE README.md Cargo.toml Cargo.lock crates
git commit -m "chore: add cross-platform release quality gates"
```

## Final Review Checklist

- [ ] The initial window is created before indexing starts.
- [ ] No initial or subsequent load pre-bakes the entire tree.
- [ ] Visible texture requests outrank off-screen work and are deduplicated.
- [ ] Memory usage and current-project disk usage never exceed configured limits after cache operations.
- [ ] Project, source, schema, and theme changes invalidate texture keys.
- [ ] Churn and texture caches are outside analyzed repositories.
- [ ] Reindex errors preserve the current project and produce a visible notification.
- [ ] `treemap.rs` is a coordinator rather than the sole owner of all UI responsibilities.
- [ ] The 1 GB per-project default is documented and tested.
- [ ] Formatting, Clippy, and all workspace tests pass.
