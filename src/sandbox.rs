// Sandbox boundary detection — ensures file operations stay within
// the project root (CODECODER_ROOT). Uses canonicalize to prevent
// `../` traversal attacks.

use std::path::{Path, PathBuf};
use serde_json::Value;

/// 判断路径是否在沙箱（CODECODER_ROOT）范围内。
/// 使用 canonicalize 规范化路径，防止 `../` 绕过。
pub fn in_sandbox(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => p.starts_with(&r),
        _ => false,
    }
}

/// 根据工具名和参数判断该工具操作是否在沙箱范围内。
/// write_file/read_file/edit_file 检查 path 参数
/// run_command 检查 cwd 参数（默认 root 在沙箱内）
/// 其他工具默认在沙箱内（非文件操作）
pub fn tool_in_sandbox(name: &str, args: &Value, root: &Path) -> bool {
    match name {
        "write_file" | "read_file" | "edit_file" => {
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => root.join(p),
                None => return true, // 无 path 参数，默认在沙箱内
            };
            in_sandbox(&path, root)
        }
        "run_command" => {
            match args.get("cwd").and_then(|v| v.as_str()) {
                Some(cwd) => {
                    let cwd_path = if cwd.starts_with('/') {
                        PathBuf::from(cwd)
                    } else {
                        root.join(cwd)
                    };
                    in_sandbox(&cwd_path, root)
                }
                None => true, // 默认 cwd = root，在沙箱内
            }
        }
        "list_directory" => {
            let path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => root.join(p),
                None => return true,
            };
            in_sandbox(&path, root)
        }
        "glob" | "grep" | "diff" => {
            // 这些工具默认在沙箱内操作，不检查路径
            true
        }
        _ => true, // 非文件操作工具默认在沙箱内
    }
}

/// 将相对路径转换为沙箱内绝对路径，并确保结果在沙箱内。
/// 返回 Err 如果路径试图逃逸沙箱。
pub fn sandbox_join(root: &Path, path: &str) -> Result<PathBuf, String> {
    let full = root.join(path);
    if in_sandbox(&full, root) {
        Ok(full)
    } else {
        Err(format!(
            "path '{}' escapes sandbox '{}'",
            full.display(),
            root.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_in_sandbox_same_dir() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello").unwrap();
        assert!(in_sandbox(&file, dir.path()));
    }

    #[test]
    fn test_in_sandbox_outside() {
        let dir = tempdir().unwrap();
        let outside = PathBuf::from("/tmp/outside.txt");
        assert!(!in_sandbox(&outside, dir.path()));
    }

    #[test]
    fn test_in_sandbox_traversal_attempt() {
        let dir = tempdir().unwrap();
        // 模拟 ../etc/passwd 逃逸
        let traversal = dir.path().join("../../etc/passwd");
        // canonicalize 会解析真实路径，不在沙箱内
        assert!(!in_sandbox(&traversal, dir.path()));
    }

    #[test]
    fn test_tool_in_sandbox_write_file_inside() {
        let dir = tempdir().unwrap();
        // Create the directory structure so canonicalize can resolve the path
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        let args = serde_json::json!({"path": "src/main.rs", "content": "fn main() {}"});
        assert!(tool_in_sandbox("write_file", &args, dir.path()));
    }

    #[test]
    fn test_tool_in_sandbox_write_file_outside() {
        let dir = tempdir().unwrap();
        let args = serde_json::json!({"path": "/tmp/escape.txt", "content": "bad"});
        assert!(!tool_in_sandbox("write_file", &args, dir.path()));
    }

    #[test]
    fn test_tool_in_sandbox_run_command_inside() {
        let dir = tempdir().unwrap();
        let args = serde_json::json!({"cmd": "npm test", "cwd": dir.path().to_str().unwrap()});
        assert!(tool_in_sandbox("run_command", &args, dir.path()));
    }

    #[test]
    fn test_sandbox_join_inside() {
        let dir = tempdir().unwrap();
        // Create the file so canonicalize can resolve it
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        let result = sandbox_join(dir.path(), "src/main.rs");
        assert!(result.is_ok());
    }

    #[test]
    fn test_sandbox_join_outside() {
        let dir = tempdir().unwrap();
        let result = sandbox_join(dir.path(), "/etc/passwd");
        assert!(result.is_err());
    }
}