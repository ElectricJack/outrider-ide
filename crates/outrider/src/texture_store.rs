use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use outrider_index::{SymbolId, SymbolKind};

use crate::rasterize::MAX_TEX_H;

const MAGIC: &[u8; 8] = b"OUTRTX01";
pub(crate) const HEADER_LEN: usize = 32;
const ACCESS_OFFSET: u64 = 24;
const MAX_TEX_W: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey(u64);

impl TextureKey {
    pub fn new(
        relative_path: &str,
        source_fingerprint: u64,
        symbol: &SymbolId,
        render_schema: u64,
        theme_fingerprint: u64,
    ) -> Self {
        let mut hash = Fnv1a::new();
        hash.field(normalize_relative_path(relative_path).as_bytes());
        hash.field(&source_fingerprint.to_le_bytes());
        hash.field(symbol_kind_bytes(&symbol.kind).as_bytes());
        hash.field(symbol.qualified_path.replace('\\', "/").as_bytes());
        hash.field(&symbol.ordinal.to_le_bytes());
        hash.field(&render_schema.to_le_bytes());
        hash.field(&theme_fingerprint.to_le_bytes());
        Self(hash.finish())
    }

    fn filename(self) -> String {
        format!("{:016x}.tex", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexturePayload {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Metadata {
    file_bytes: u64,
    last_access: u64,
}

pub struct TextureStore {
    dir: PathBuf,
    _namespace_resources: Arc<NamespaceResources>,
    namespace_lock: Arc<Mutex<()>>,
    namespace_usage: Arc<AtomicU64>,
    namespace_generation: Arc<AtomicU64>,
    generation: u64,
    max_bytes: u64,
    used_bytes: u64,
    clock: u64,
    entries: HashMap<TextureKey, Metadata>,
}

impl TextureStore {
    pub(crate) fn prepare_replacement(
        project_root: &Path,
        max_bytes: u64,
    ) -> io::Result<PreparedTextureStore> {
        let cache_root = dirs::cache_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cache directory unavailable"))?
            .join("outrider")
            .join("textures");
        Self::prepare_project_replacement_at(&cache_root, project_root, max_bytes)
    }

    fn prepare_project_replacement_at(
        cache_root: &Path,
        project_root: &Path,
        max_bytes: u64,
    ) -> io::Result<PreparedTextureStore> {
        let identity = canonical_project_identity(project_root);
        Self::prepare_namespace(namespace_derivation(cache_root, &identity), max_bytes)
    }

    #[cfg(test)]
    pub(crate) fn prepare_replacement_at(
        cache_root: &Path,
        project_identity: &str,
        max_bytes: u64,
    ) -> io::Result<PreparedTextureStore> {
        Self::prepare_namespace(
            namespace_derivation(cache_root, project_identity),
            max_bytes,
        )
    }

    fn prepare_namespace(
        namespace: NamespaceDerivation,
        max_bytes: u64,
    ) -> io::Result<PreparedTextureStore> {
        let generation = namespace.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(PreparedTextureStore {
            namespace,
            max_bytes,
            generation,
        })
    }

    #[cfg(test)]
    pub fn open_at(cache_root: &Path, project_identity: &str, max_bytes: u64) -> io::Result<Self> {
        let namespace = namespace_derivation(cache_root, project_identity);
        let generation = namespace.shared.generation.load(Ordering::Acquire);
        Self::open_namespace(namespace, max_bytes, generation)
    }

    fn open_namespace(
        namespace: NamespaceDerivation,
        max_bytes: u64,
        generation: u64,
    ) -> io::Result<Self> {
        let dir = namespace.dir;
        fs::create_dir_all(&dir)?;
        let dir = fs::canonicalize(&dir)?;
        let namespace_lock = Arc::clone(&namespace.shared.lock);
        let namespace_usage = Arc::clone(&namespace.shared.usage);
        let namespace_generation = Arc::clone(&namespace.shared.generation);
        let operation_lock = Arc::clone(&namespace_lock);
        let _guard = operation_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut store = Self {
            dir,
            _namespace_resources: namespace.shared,
            namespace_lock,
            namespace_usage,
            namespace_generation,
            generation,
            max_bytes,
            used_bytes: 0,
            clock: 0,
            entries: HashMap::new(),
        };
        store.ensure_current_generation()?;
        store.rebuild_index_unlocked()?;
        store.evict_unlocked()?;
        Ok(store)
    }

    pub fn load(&mut self, key: &TextureKey) -> io::Result<Option<TexturePayload>> {
        self.load_with_remove(key, |path| fs::remove_file(path))
    }

    fn load_with_remove(
        &mut self,
        key: &TextureKey,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<Option<TexturePayload>> {
        let operation_lock = Arc::clone(&self.namespace_lock);
        let _guard = operation_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure_current_generation()?;
        self.rebuild_index_with_remove_unlocked(&mut remove)?;
        let path = self.path(*key);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.remove_metadata(*key);
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let Some(header) = validate_file(&mut file, self.max_bytes)? else {
            drop(file);
            match remove(&path) {
                Ok(()) => self.remove_metadata(*key),
                Err(error) if error.kind() == io::ErrorKind::NotFound => self.remove_metadata(*key),
                Err(error) => {
                    self.rebuild_index_with_remove_unlocked(|_| {
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "cleanup disabled",
                        ))
                    })?;
                    return Err(error);
                }
            }
            return Ok(None);
        };
        let len = usize::try_from(header.payload_len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "texture is too large"))?;
        let mut bytes = vec![0; len];
        file.read_exact(&mut bytes)?;
        drop(file);

        self.clock = self.clock.max(header.last_access);
        let access = self.next_access();
        let mut file = OpenOptions::new().write(true).open(&path)?;
        file.seek(SeekFrom::Start(ACCESS_OFFSET))?;
        file.write_all(&access.to_le_bytes())?;
        file.flush()?;
        if let Some(previous) = self.entries.insert(
            *key,
            Metadata {
                file_bytes: header.file_bytes,
                last_access: access,
            },
        ) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.file_bytes);
        }
        self.used_bytes = self.used_bytes.saturating_add(header.file_bytes);
        self.publish_usage();
        self.evict_unlocked()?;
        Ok(Some(TexturePayload {
            width: header.width,
            height: header.height,
            bytes,
        }))
    }

    pub fn save(&mut self, key: &TextureKey, payload: &TexturePayload) -> io::Result<()> {
        self.save_with_io(key, payload, replace_file_atomically, |path| {
            fs::remove_file(path)
        })
    }

    fn save_with_io(
        &mut self,
        key: &TextureKey,
        payload: &TexturePayload,
        replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let operation_lock = Arc::clone(&self.namespace_lock);
        let _guard = operation_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure_current_generation()?;
        let payload_len = validate_payload(payload)?;
        let file_bytes = HEADER_LEN as u64 + payload_len;
        if file_bytes > self.max_bytes {
            return Ok(());
        }
        self.rebuild_index_with_remove_unlocked(&mut remove)?;
        self.reserve_physical_space(file_bytes, &mut remove)?;
        let access = self.next_access();
        let path = self.path(*key);
        let temp = path.with_extension("tmp");
        let result = (|| {
            let mut file = File::create(&temp)?;
            write_header(
                &mut file,
                payload.width,
                payload.height,
                payload_len,
                access,
            )?;
            file.write_all(&payload.bytes)?;
            file.flush()?;
            file.sync_all()?;
            replace(&temp, &path)
        })();
        if result.is_err() {
            let _ = remove(&temp);
            self.rebuild_index_with_remove_unlocked(&mut remove)?;
        }
        result?;

        if let Some(old) = self.entries.insert(
            *key,
            Metadata {
                file_bytes,
                last_access: access,
            },
        ) {
            self.used_bytes = self.used_bytes.saturating_sub(old.file_bytes);
        }
        self.used_bytes = self.used_bytes.saturating_add(file_bytes);
        self.publish_usage();
        self.evict_unlocked()
    }

    #[cfg(test)]
    fn save_with_replace_for_test(
        &mut self,
        key: &TextureKey,
        payload: &TexturePayload,
        replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.save_with_io(key, payload, replace, |path| fs::remove_file(path))
    }

    #[cfg(test)]
    fn save_with_io_for_test(
        &mut self,
        key: &TextureKey,
        payload: &TexturePayload,
        replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
        remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.save_with_io(key, payload, replace, remove)
    }

    #[allow(dead_code)] // Public store API; UI wiring lands in a later task.
    pub fn clear(&mut self) -> io::Result<()> {
        self.clear_with_remove(|path| fs::remove_file(path))
    }

    fn clear_with_remove(
        &mut self,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let operation_lock = Arc::clone(&self.namespace_lock);
        let _guard = operation_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure_current_generation()?;
        let mut first_error = None;
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("tex" | "tmp")
            ) {
                if let Err(error) = remove(&path) {
                    if error.kind() != io::ErrorKind::NotFound && first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        self.rebuild_index_with_remove_unlocked(|_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cleanup disabled",
            ))
        })?;
        first_error.map_or(Ok(()), Err)
    }

    #[allow(dead_code)] // Public store API; UI wiring lands in a later task.
    pub fn used_bytes(&self) -> u64 {
        self.namespace_usage.load(Ordering::Acquire)
    }

    fn rebuild_index_unlocked(&mut self) -> io::Result<()> {
        self.rebuild_index_with_remove_unlocked(|path| fs::remove_file(path))
    }

    fn rebuild_index_with_remove_unlocked(
        &mut self,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.entries.clear();
        self.used_bytes = 0;
        self.clock = 0;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            let extension = path.extension().and_then(|value| value.to_str());
            if extension == Some("tmp") {
                let file_bytes = entry.metadata()?.len();
                if let Err(error) = remove(&path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        self.used_bytes = self.used_bytes.saturating_add(file_bytes);
                    }
                }
                continue;
            }
            if extension != Some("tex") {
                continue;
            }
            let file_bytes = entry.metadata()?.len();
            let Some(key) = key_from_path(&path) else {
                if let Err(error) = remove(&path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        self.used_bytes = self.used_bytes.saturating_add(file_bytes);
                    }
                }
                continue;
            };
            let mut file = File::open(&path)?;
            match validate_file(&mut file, self.max_bytes)? {
                Some(header) => {
                    self.clock = self.clock.max(header.last_access);
                    self.used_bytes = self.used_bytes.saturating_add(header.file_bytes);
                    self.entries.insert(
                        key,
                        Metadata {
                            file_bytes: header.file_bytes,
                            last_access: header.last_access,
                        },
                    );
                }
                None => {
                    drop(file);
                    if let Err(error) = remove(&path) {
                        if error.kind() != io::ErrorKind::NotFound {
                            self.used_bytes = self.used_bytes.saturating_add(file_bytes);
                        }
                    }
                }
            }
        }
        self.publish_usage();
        Ok(())
    }

    fn reserve_physical_space(
        &mut self,
        file_bytes: u64,
        remove: &mut impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        while self.used_bytes.saturating_add(file_bytes) > self.max_bytes {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(key, metadata)| (metadata.last_access, key.0))
                .map(|(key, _)| *key)
            else {
                return Err(io::Error::other("unable to reserve texture cache space"));
            };
            match remove(&self.path(victim)) {
                Ok(()) => self.remove_metadata(victim),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.remove_metadata(victim)
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn evict_unlocked(&mut self) -> io::Result<()> {
        while self.used_bytes > self.max_bytes {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(key, metadata)| (metadata.last_access, key.0))
                .map(|(key, _)| *key)
            else {
                break;
            };
            match fs::remove_file(self.path(victim)) {
                Ok(()) => self.remove_metadata(victim),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.remove_metadata(victim)
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn remove_metadata(&mut self, key: TextureKey) {
        if let Some(metadata) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(metadata.file_bytes);
            self.publish_usage();
        }
    }

    fn publish_usage(&self) {
        self.namespace_usage
            .store(self.used_bytes, Ordering::Release);
    }

    fn ensure_current_generation(&self) -> io::Result<()> {
        if self.namespace_generation.load(Ordering::Acquire) != self.generation {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "texture cache namespace was replaced",
            ));
        }
        Ok(())
    }

    pub(crate) fn shared_usage(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.namespace_usage)
    }

    fn next_access(&mut self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(u64::MAX as u128) as u64;
        self.clock = now.max(self.clock.saturating_add(1));
        self.clock
    }

    fn path(&self, key: TextureKey) -> PathBuf {
        self.dir.join(key.filename())
    }

    #[cfg(test)]
    fn write_raw_for_test(&self, key: &TextureKey, bytes: &[u8]) -> io::Result<()> {
        fs::write(self.path(*key), bytes)
    }

    #[cfg(test)]
    fn append_raw_for_test(&self, key: &TextureKey, bytes: &[u8]) -> io::Result<()> {
        OpenOptions::new()
            .append(true)
            .open(self.path(*key))?
            .write_all(bytes)
    }

    #[cfg(test)]
    pub(crate) fn dir_for_test(&self) -> &Path {
        &self.dir
    }

    #[cfg(test)]
    fn rebuild_index_with_remove_for_test(
        &mut self,
        remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let operation_lock = Arc::clone(&self.namespace_lock);
        let _guard = operation_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.rebuild_index_with_remove_unlocked(remove)
    }

    #[cfg(test)]
    fn load_with_remove_for_test(
        &mut self,
        key: &TextureKey,
        remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<Option<TexturePayload>> {
        self.load_with_remove(key, remove)
    }

    #[cfg(test)]
    fn clear_with_remove_for_test(
        &mut self,
        remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.clear_with_remove(remove)
    }
}

pub(crate) struct PreparedTextureStore {
    namespace: NamespaceDerivation,
    max_bytes: u64,
    generation: u64,
}

impl PreparedTextureStore {
    pub(crate) fn open(self) -> io::Result<TextureStore> {
        TextureStore::open_namespace(self.namespace, self.max_bytes, self.generation)
    }
}

struct NamespaceDerivation {
    dir: PathBuf,
    shared: Arc<NamespaceResources>,
}

struct NamespaceResources {
    lock: Arc<Mutex<()>>,
    usage: Arc<AtomicU64>,
    generation: Arc<AtomicU64>,
}

fn canonical_project_identity(project_root: &Path) -> String {
    let canonical = fs::canonicalize(project_root).unwrap_or_else(|_| {
        std::path::absolute(project_root).unwrap_or_else(|_| project_root.into())
    });
    let mut identity = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    identity.make_ascii_lowercase();
    identity
}

fn namespace_derivation(cache_root: &Path, project_identity: &str) -> NamespaceDerivation {
    let mut hash = Fnv1a::new();
    hash.field(project_identity.replace('\\', "/").as_bytes());
    let root = std::path::absolute(cache_root).unwrap_or_else(|_| cache_root.to_path_buf());
    let dir = root.join(format!("{:016x}", hash.finish()));
    let mut key = dir.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    key.make_ascii_lowercase();
    NamespaceDerivation {
        dir,
        shared: namespace_resources(key),
    }
}

fn namespace_resources(key: String) -> Arc<NamespaceResources> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Weak<NamespaceResources>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap_or_else(|error| error.into_inner());
    locks.retain(|_, resources| resources.strong_count() > 0);
    if let Some(resources) = locks.get(&key).and_then(Weak::upgrade) {
        return resources;
    }
    let resources = Arc::new(NamespaceResources {
        lock: Arc::new(Mutex::new(())),
        usage: Arc::new(AtomicU64::new(0)),
        generation: Arc::new(AtomicU64::new(1)),
    });
    locks.insert(key, Arc::downgrade(&resources));
    resources
}

#[cfg(unix)]
fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic file replacement is unsupported on this platform",
        ));
    }
    fs::rename(source, destination)
}

struct Header {
    width: u32,
    height: u32,
    payload_len: u64,
    last_access: u64,
    file_bytes: u64,
}

fn validate_file(file: &mut File, max_bytes: u64) -> io::Result<Option<Header>> {
    let file_bytes = file.metadata()?.len();
    if file_bytes < HEADER_LEN as u64 || file_bytes > max_bytes {
        return Ok(None);
    }
    let mut raw = [0; HEADER_LEN];
    if file.read_exact(&mut raw).is_err() || &raw[..8] != MAGIC {
        return Ok(None);
    }
    let width = u32::from_le_bytes(raw[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(raw[12..16].try_into().unwrap());
    let payload_len = u64::from_le_bytes(raw[16..24].try_into().unwrap());
    let last_access = u64::from_le_bytes(raw[24..32].try_into().unwrap());
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4));
    if width == 0
        || height == 0
        || width > MAX_TEX_W
        || height > MAX_TEX_H as u32
        || expected != Some(payload_len)
        || HEADER_LEN as u64 + payload_len != file_bytes
    {
        return Ok(None);
    }
    Ok(Some(Header {
        width,
        height,
        payload_len,
        last_access,
        file_bytes,
    }))
}

fn validate_payload(payload: &TexturePayload) -> io::Result<u64> {
    let len = u64::try_from(payload.bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "texture is too large"))?;
    let expected = u64::from(payload.width)
        .checked_mul(u64::from(payload.height))
        .and_then(|pixels| pixels.checked_mul(4));
    if payload.width == 0
        || payload.height == 0
        || payload.width > MAX_TEX_W
        || payload.height > MAX_TEX_H as u32
        || expected != Some(len)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid texture dimensions or payload length",
        ));
    }
    Ok(len)
}

fn write_header(
    writer: &mut impl Write,
    width: u32,
    height: u32,
    payload_len: u64,
    last_access: u64,
) -> io::Result<()> {
    writer.write_all(MAGIC)?;
    writer.write_all(&width.to_le_bytes())?;
    writer.write_all(&height.to_le_bytes())?;
    writer.write_all(&payload_len.to_le_bytes())?;
    writer.write_all(&last_access.to_le_bytes())
}

fn key_from_path(path: &Path) -> Option<TextureKey> {
    if path.extension()?.to_str()? != "tex" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    (stem.len() == 16)
        .then(|| u64::from_str_radix(stem, 16).ok())
        .flatten()
        .map(TextureKey)
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn symbol_kind_bytes(kind: &SymbolKind) -> String {
    match kind {
        SymbolKind::Folder => "folder".into(),
        SymbolKind::File => "file".into(),
        SymbolKind::Chunk => "chunk".into(),
        SymbolKind::Item { label } => format!("item:{label}"),
    }
}

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn field(&mut self, bytes: &[u8]) {
        self.update(&(bytes.len() as u64).to_le_bytes());
        self.update(bytes);
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{TextureKey, TexturePayload, TextureStore, HEADER_LEN};
    use outrider_index::{SymbolId, SymbolKind};

    fn key(path: &str, source_fingerprint: u64) -> TextureKey {
        TextureKey::new(
            path,
            source_fingerprint,
            &SymbolId {
                kind: SymbolKind::Item { label: "fn".into() },
                qualified_path: format!("{path}::item"),
                ordinal: 0,
            },
            1,
            2,
        )
    }

    fn payload(bytes: usize) -> TexturePayload {
        assert_eq!(bytes % 4, 0);
        TexturePayload {
            width: (bytes / 4) as u32,
            height: 1,
            bytes: vec![0x5a; bytes],
        }
    }

    #[test]
    fn identical_symbols_in_different_projects_do_not_share_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut one = TextureStore::open_at(dir.path(), "project-one", 1024).unwrap();
        let mut two = TextureStore::open_at(dir.path(), "project-two", 1024).unwrap();
        one.save(&key("src/lib.rs", 11), &payload(16)).unwrap();
        assert!(two.load(&key("src/lib.rs", 11)).unwrap().is_none());
    }

    #[test]
    fn source_fingerprint_changes_the_cache_key() {
        assert_ne!(key("src/lib.rs", 11), key("src/lib.rs", 12));
    }

    #[test]
    fn normalized_relative_paths_share_a_cache_key() {
        assert_eq!(key("src\\lib.rs", 11), key("src/lib.rs", 11));
    }

    #[test]
    fn symbol_schema_and_theme_each_change_the_cache_key() {
        let base = key("src/lib.rs", 11);
        let symbol = SymbolId {
            kind: SymbolKind::Item { label: "fn".into() },
            qualified_path: "src/lib.rs::other".into(),
            ordinal: 0,
        };
        assert_ne!(base, TextureKey::new("src/lib.rs", 11, &symbol, 1, 2));
        assert_ne!(
            base,
            TextureKey::new(
                "src/lib.rs",
                11,
                &SymbolId {
                    kind: SymbolKind::Item { label: "fn".into() },
                    qualified_path: "src/lib.rs::item".into(),
                    ordinal: 0,
                },
                2,
                2,
            )
        );
        assert_ne!(
            base,
            TextureKey::new(
                "src/lib.rs",
                11,
                &SymbolId {
                    kind: SymbolKind::Item { label: "fn".into() },
                    qualified_path: "src/lib.rs::item".into(),
                    ordinal: 0,
                },
                1,
                3,
            )
        );
    }

    #[test]
    fn corrupt_length_is_rejected_without_allocation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TextureStore::open_at(dir.path(), "project", 1024).unwrap();
        store
            .write_raw_for_test(&key("a.rs", 1), &[0xff; 12])
            .unwrap();
        assert!(store.load(&key("a.rs", 1)).unwrap().is_none());
    }

    #[test]
    fn invalid_dimensions_and_trailing_bytes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TextureStore::open_at(dir.path(), "project", 4096).unwrap();
        let invalid = TexturePayload {
            width: 1025,
            height: 1,
            bytes: vec![0; 1025 * 4],
        };
        assert!(store.save(&key("wide.rs", 1), &invalid).is_err());

        store.save(&key("ok.rs", 1), &payload(16)).unwrap();
        store
            .append_raw_for_test(&key("ok.rs", 1), &[0xaa])
            .unwrap();
        assert!(store.load(&key("ok.rs", 1)).unwrap().is_none());
    }

    #[test]
    fn saving_past_limit_evicts_oldest_entry() {
        let dir = tempfile::tempdir().unwrap();
        let limit = (HEADER_LEN + 24 + HEADER_LEN + 24 - 1) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", limit).unwrap();
        store.save(&key("old.rs", 1), &payload(24)).unwrap();
        store.save(&key("new.rs", 1), &payload(24)).unwrap();
        assert!(store.load(&key("old.rs", 1)).unwrap().is_none());
        assert!(store.load(&key("new.rs", 1)).unwrap().is_some());
        assert!(store.used_bytes() <= limit);
    }

    #[test]
    fn failed_pre_eviction_never_writes_past_the_hard_limit() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        store.save(&key("old.rs", 1), &payload(16)).unwrap();

        let result = store.save_with_io_for_test(
            &key("new.rs", 1),
            &payload(16),
            |_, _| Ok(()),
            |_| Err(std::io::Error::other("injected eviction failure")),
        );

        assert!(result.is_err());
        let physical_bytes: u64 = std::fs::read_dir(&store.dir)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert!(physical_bytes <= entry_bytes);
        assert!(store.used_bytes() <= entry_bytes);
        assert!(store.load(&key("old.rs", 1)).unwrap().is_some());
        assert!(store.load(&key("new.rs", 1)).unwrap().is_none());
    }

    #[test]
    fn undeletable_invalid_entry_counts_toward_the_hard_limit() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        store
            .write_raw_for_test(&key("broken.rs", 1), &[0xff; HEADER_LEN + 8])
            .unwrap();

        store
            .rebuild_index_with_remove_for_test(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected cleanup failure",
                ))
            })
            .unwrap();

        assert_eq!(store.used_bytes(), (HEADER_LEN + 8) as u64);
        assert!(store
            .save_with_io_for_test(
                &key("new.rs", 1),
                &payload(16),
                |_, _| Ok(()),
                |_| Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected cleanup failure"
                )),
            )
            .is_err());
        assert!(namespace_bytes(&store) <= entry_bytes);
    }

    #[test]
    fn undeletable_temp_entry_counts_toward_the_hard_limit() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        std::fs::write(store.dir.join("abandoned.tmp"), vec![0xff; HEADER_LEN + 8]).unwrap();

        store
            .rebuild_index_with_remove_for_test(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected cleanup failure",
                ))
            })
            .unwrap();

        assert_eq!(store.used_bytes(), (HEADER_LEN + 8) as u64);
        assert!(store
            .save_with_io_for_test(
                &key("new.rs", 1),
                &payload(16),
                |_, _| Ok(()),
                |_| Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected cleanup failure"
                )),
            )
            .is_err());
        assert!(namespace_bytes(&store) <= entry_bytes);
    }

    #[test]
    fn invalid_load_cleanup_failure_preserves_physical_occupancy() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", entry_bytes * 2).unwrap();
        let cache_key = key("broken.rs", 1);
        store.save(&cache_key, &payload(16)).unwrap();
        store.append_raw_for_test(&cache_key, &[0xff; 4]).unwrap();

        let result = store.load_with_remove_for_test(&cache_key, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected cleanup failure",
            ))
        });

        assert!(result.is_err());
        assert_eq!(store.used_bytes(), entry_bytes + 4);
        assert!(namespace_bytes(&store) <= entry_bytes * 2);
    }

    #[test]
    fn overlapping_same_project_stores_never_exceed_the_namespace_limit() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let one = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        let two = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        let start = Arc::new(Barrier::new(2));
        let one_start = Arc::clone(&start);
        let two_start = Arc::clone(&start);

        let one = std::thread::spawn(move || {
            let mut store = one;
            one_start.wait();
            for ordinal in 0..64 {
                store
                    .save(&key(&format!("one-{ordinal}.rs"), 1), &payload(16))
                    .unwrap();
            }
            store.used_bytes()
        });
        let two = std::thread::spawn(move || {
            let mut store = two;
            two_start.wait();
            for ordinal in 0..64 {
                store
                    .save(&key(&format!("two-{ordinal}.rs"), 1), &payload(16))
                    .unwrap();
            }
            store.used_bytes()
        });

        assert!(one.join().unwrap() <= entry_bytes);
        assert!(two.join().unwrap() <= entry_bytes);
        let reopened = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        assert!(namespace_bytes(&reopened) <= entry_bytes);
        assert_eq!(reopened.used_bytes(), namespace_bytes(&reopened));
    }

    #[test]
    fn replaced_generation_cannot_write_after_reduced_limit_reconciliation() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut old = TextureStore::open_at(dir.path(), "project", entry_bytes * 2).unwrap();
        old.save(&key("old.rs", 1), &payload(16)).unwrap();

        let replacement =
            TextureStore::prepare_replacement_at(dir.path(), "project", entry_bytes).unwrap();
        let replacement = replacement.open().unwrap();
        assert!(replacement.used_bytes() <= entry_bytes);

        assert!(old.save(&key("stale.rs", 1), &payload(16)).is_err());
        assert!(namespace_bytes(&replacement) <= entry_bytes);
        assert_eq!(replacement.used_bytes(), namespace_bytes(&replacement));
    }

    #[test]
    fn prepared_project_open_preserves_canonical_on_disk_namespace_identity() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&project).unwrap();
        let mut canonical = std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        #[cfg(windows)]
        canonical.make_ascii_lowercase();
        let direct = TextureStore::open_at(&cache, &canonical, 1024).unwrap();

        let prepared =
            TextureStore::prepare_project_replacement_at(&cache, &project, 1024).unwrap();
        let replacement = prepared.open().unwrap();

        assert_eq!(direct.dir_for_test(), replacement.dir_for_test());
    }

    #[test]
    fn canonical_alias_replacement_shares_generation_and_fences_old_store() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let alias = dir.path().join("alias");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&project).unwrap();
        if create_dir_symlink_for_test(&project, &alias).is_err() {
            return;
        }
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut old =
            TextureStore::prepare_project_replacement_at(&cache, &project, entry_bytes * 2)
                .unwrap()
                .open()
                .unwrap();
        old.save(&key("old.rs", 1), &payload(16)).unwrap();

        let replacement = TextureStore::prepare_project_replacement_at(&cache, &alias, entry_bytes)
            .unwrap()
            .open()
            .unwrap();

        assert_eq!(old.dir_for_test(), replacement.dir_for_test());
        assert!(old.save(&key("stale.rs", 1), &payload(16)).is_err());
        assert!(namespace_bytes(&replacement) <= entry_bytes);
    }

    #[cfg(unix)]
    fn create_dir_symlink_for_test(
        source: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(source, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink_for_test(
        source: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(source, link)
    }

    #[test]
    fn failed_atomic_replacement_preserves_old_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TextureStore::open_at(dir.path(), "project", 1024).unwrap();
        let cache_key = key("a.rs", 1);
        let old = payload(16);
        let new = payload(20);
        store.save(&cache_key, &old).unwrap();

        let result = store.save_with_replace_for_test(&cache_key, &new, |_, _| {
            Err(std::io::Error::other("injected replacement failure"))
        });

        assert!(result.is_err());
        assert_eq!(store.load(&cache_key).unwrap(), Some(old));
        store.save(&cache_key, &new).unwrap();
        assert_eq!(store.load(&cache_key).unwrap(), Some(new));
    }

    #[test]
    fn load_of_entry_created_after_open_updates_actual_usage_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut one = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        let mut two = TextureStore::open_at(dir.path(), "project", entry_bytes).unwrap();
        let external = key("external.rs", 1);
        two.save(&external, &payload(16)).unwrap();

        assert!(one.load(&external).unwrap().is_some());
        assert_eq!(one.used_bytes(), entry_bytes);

        let local = key("local.rs", 1);
        one.save(&local, &payload(16)).unwrap();
        assert!(one.used_bytes() <= entry_bytes);
        assert!(one.load(&external).unwrap().is_none());
        assert!(one.load(&local).unwrap().is_some());
    }

    #[test]
    fn successful_load_persists_lru_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let project = "project";
        let mut store = TextureStore::open_at(dir.path(), project, entry_bytes * 2).unwrap();
        store.save(&key("a.rs", 1), &payload(16)).unwrap();
        store.save(&key("b.rs", 1), &payload(16)).unwrap();
        assert!(store.load(&key("a.rs", 1)).unwrap().is_some());
        drop(store);

        let mut reopened = TextureStore::open_at(dir.path(), project, entry_bytes * 2).unwrap();
        reopened.save(&key("c.rs", 1), &payload(16)).unwrap();
        assert!(reopened.load(&key("a.rs", 1)).unwrap().is_some());
        assert!(reopened.load(&key("b.rs", 1)).unwrap().is_none());
    }

    #[test]
    fn clear_removes_only_the_current_project_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut one = TextureStore::open_at(dir.path(), "one", 1024).unwrap();
        let mut two = TextureStore::open_at(dir.path(), "two", 1024).unwrap();
        one.save(&key("a.rs", 1), &payload(16)).unwrap();
        two.save(&key("a.rs", 1), &payload(16)).unwrap();
        one.clear().unwrap();
        assert!(one.load(&key("a.rs", 1)).unwrap().is_none());
        assert!(two.load(&key("a.rs", 1)).unwrap().is_some());
    }

    #[test]
    fn partial_clear_reconciles_usage_to_files_that_remain() {
        let dir = tempfile::tempdir().unwrap();
        let entry_bytes = (HEADER_LEN + 16) as u64;
        let mut store = TextureStore::open_at(dir.path(), "project", entry_bytes * 2).unwrap();
        let preserved = key("preserved.rs", 1);
        store.save(&preserved, &payload(16)).unwrap();
        store.save(&key("deleted.rs", 1), &payload(16)).unwrap();
        let preserved_path = store.path(preserved);

        let result = store.clear_with_remove_for_test(|path| {
            if path == preserved_path {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected clear failure",
                ))
            } else {
                std::fs::remove_file(path)
            }
        });

        assert!(result.is_err());
        assert_eq!(store.used_bytes(), entry_bytes);
        assert_eq!(namespace_bytes(&store), entry_bytes);
    }

    fn namespace_bytes(store: &TextureStore) -> u64 {
        std::fs::read_dir(&store.dir)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum()
    }
}
