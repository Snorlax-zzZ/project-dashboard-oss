#![cfg(windows)]
#[path = "../src/windows_files.rs"]
mod windows_files;
use std::path::Path;

#[test]
fn reads_unicode_file_and_rejects_hardlinks_and_budget() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("win-file-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("中文 空格.md");
    std::fs::write(&path, "正文").unwrap();
    assert_eq!(windows_files::read(&path, 100).unwrap(), "正文".as_bytes());
    assert!(windows_files::read(&path, 1).is_err());
    let linked = root.join("linked.md");
    std::fs::hard_link(&path, &linked).unwrap();
    assert!(windows_files::read(&path, 100).is_err());
    std::fs::remove_file(linked).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn rejects_ambiguous_or_redirected_names_before_access() {
    for path in [
        "C:relative",
        "\\\\server\\share\\file",
        "\\\\?\\C:\\file",
        "C:\\dir\\..\\file",
        "C:\\file:stream",
        "C:\\NUL.txt",
        "C:\\tail.\\file",
        "C:\\tail ",
    ] {
        assert!(
            windows_files::validate_lexical(Path::new(path)).is_err(),
            "{path}"
        );
    }
    assert!(
        windows_files::validate_lexical(Path::new(&format!("F:\\{}", "a".repeat(260)))).is_err()
    );
}

#[test]
fn pinned_file_blocks_content_replacement_and_ancestor_rename() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("win-pin-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("parent")).unwrap();
    let path = root.join("parent/file.md");
    std::fs::write(&path, "before").unwrap();
    let pin = windows_files::pin_file(&path).unwrap();
    assert!(std::fs::write(&path, "after").is_err());
    assert!(std::fs::rename(root.join("parent"), root.join("moved")).is_err());
    drop(pin);
    std::fs::rename(root.join("parent"), root.join("moved")).unwrap();
    assert_eq!(
        windows_files::read(&root.join("moved/file.md"), 20).unwrap(),
        b"before"
    );
    std::fs::remove_file(root.join("moved/file.md")).unwrap();
    std::fs::remove_dir(root.join("moved")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

// 2.5：真实 NTFS junction 出现在祖先链上时必须整链拒绝，且拒绝不得穿透访问或改写目标。
// junction 用 `mklink /J` 创建（无需特权），区别于需要开发者模式的符号链接。
#[test]
fn rejects_real_junction_ancestor_and_keeps_target_untouched() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("win-junction-{}", uuid::Uuid::new_v4()));
    let real = root.join("real");
    std::fs::create_dir_all(real.join("docs")).unwrap();
    let target_file = real.join("docs/正文.md");
    std::fs::write(&target_file, "内容").unwrap();
    let link = root.join("link");
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&real)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "前置：junction 必须创建成功：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // junction 祖先：文件读取 / 目录 pin / data-root 祖先校验全部拒绝。
    assert!(windows_files::read(&link.join("docs/正文.md"), 100).is_err());
    assert!(windows_files::pin_directory(&link).is_err());
    assert!(windows_files::pin_file(&link).is_err());
    assert!(windows_files::validate_existing_parent(&link.join("新/deeper")).is_err());
    // 目标不变：指向原样、内容原样，且不经 junction 的真实路径仍可读（拒绝源于重解析而非文件本身）。
    assert_eq!(std::fs::read_link(&link).unwrap(), real);
    assert_eq!(std::fs::read(&target_file).unwrap(), "内容".as_bytes());
    assert_eq!(
        windows_files::read(&real.join("docs/正文.md"), 100).unwrap(),
        "内容".as_bytes()
    );
    std::fs::remove_dir(&link).unwrap();
    std::fs::remove_file(&target_file).unwrap();
    std::fs::remove_dir(real.join("docs")).unwrap();
    std::fs::remove_dir(&real).unwrap();
    std::fs::remove_dir(&root).unwrap();
}
