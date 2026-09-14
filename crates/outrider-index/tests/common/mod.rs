use std::fs;
use std::path::Path;

/// Copy a fixture repo to a temp dir, renaming `_gitignore` -> `.gitignore`.
pub fn copy_fixture(name: &str) -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src, dir.path());
    let marker = dir.path().join("_gitignore");
    if marker.exists() {
        fs::rename(&marker, dir.path().join(".gitignore")).unwrap();
    }
    dir
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}
