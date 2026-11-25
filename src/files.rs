//! File write helpers with atomic guarantees.

use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to a unique temporary file and atomically renames it over `path`.
/// Preserves symlink targets and prevents file corruption from partial writes or crashes.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    // Write through symlinks (for dotfile managers) rather than overwriting the symlink itself
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(directory) = target.parent() {
        fs::create_dir_all(directory)?;
    }
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let extension = format!("tmp.{}.{sequence}", process::id());
    let temporary = target.with_added_extension(extension);
    let result = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ScratchDir;

    #[test]
    fn test_write_atomically_creates_and_replaces_without_leaving_temp_files() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("nested").join("data.json");

        write_atomically(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        write_atomically(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_write_atomically_writes_through_a_symlink() {
        use std::os::unix::fs as unix_fs;

        let scratch = ScratchDir::create();
        let real = scratch.path().join("real.json");
        let link = scratch.path().join("link.json");
        fs::write(&real, b"old").unwrap();
        unix_fs::symlink(&real, &link).unwrap();

        write_atomically(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }
}
