use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
pub struct InstanceFile {
    #[allow(dead_code)]
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub project_root: String,
}

/// Find the project root by walking up from cwd.
pub fn find_project_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Ok(std::fs::canonicalize(&path).unwrap_or(path));
        }
        return Err(format!("--project {p}: not a directory"));
    }
    let mut dir = std::env::current_dir().map_err(|e| format!("cannot get cwd: {e}"))?;
    loop {
        if dir.join(".outrider").is_dir() || dir.join(".git").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("no .git or .outrider directory found (use --project)".into());
        }
    }
}

/// Compute the same project identity hash as the app (FNV-1a of canonical path).
pub fn project_identity_hash(project_root: &Path) -> String {
    let canonical =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let mut identity = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    identity.make_ascii_lowercase();

    // FNV-1a hash (same algorithm as texture_store.rs)
    let mut h: u64 = 0xcbf29ce484222325;
    let bytes = identity.as_bytes();
    // field() prepends length
    let len_bytes = (bytes.len() as u64).to_le_bytes();
    for byte in &len_bytes {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x100000001b3);
    }
    for byte in bytes {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Load the instance file for a project.
pub fn load_instance(project_root: &Path) -> Result<InstanceFile, String> {
    let env_override = std::env::var("OUTRIDER_INSTANCE").ok();
    let path = if let Some(p) = env_override {
        PathBuf::from(p)
    } else {
        let cache = dirs::cache_dir().ok_or("cannot determine cache directory")?;
        let hash = project_identity_hash(project_root);
        cache
            .join("outrider")
            .join("instances")
            .join(format!("{hash}.json"))
    };
    if !path.exists() {
        return Err(format!(
            "no running outrider for {}; start the app or use --offline",
            project_root.display()
        ));
    }
    let json =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read instance file: {e}"))?;
    serde_json::from_str(&json).map_err(|e| format!("invalid instance file: {e}"))
}
