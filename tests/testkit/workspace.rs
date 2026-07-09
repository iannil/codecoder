// tests/testkit/workspace.rs — throwaway temp-dir project root for black-box runs.
use std::path::PathBuf;
use std::process::Command;

pub struct Workspace {
    pub dir: tempfile::TempDir,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    pub fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }
    pub fn write(&self, rel: &str, contents: &str) {
        let p = self.root().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root().join(rel)).unwrap()
    }
    pub fn exists(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
    }
    pub fn git_init(&self) {
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(self.root())
                .status()
                .unwrap();
        }
    }
}
