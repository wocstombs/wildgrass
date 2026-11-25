//! Shared test utilities and fixtures.

use std::path::{Path, PathBuf};
use std::{env, fs};

use uuid::Uuid;

/// RAII temporary directory removed on drop. Prevents test pollution in real config/cache directories.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn create() -> Self {
        let directory = env::temp_dir().join(format!("wildgrass-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `count` lines of `text` in `.lrc` form, one every 8 seconds.
pub fn lrc_lines(count: u32, text: &str) -> String {
    (1..=count)
        .map(|i| format!("[{:02}:{:02}.00]{text} {i}\n", i * 8 / 60, i * 8 % 60))
        .collect()
}
