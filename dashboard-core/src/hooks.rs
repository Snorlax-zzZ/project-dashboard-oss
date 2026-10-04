// M3 §11 四层同步之层 1：post-commit hook 安装 / 卸载 / 幂等追加。
// hook 用 `# BEGIN project-dashboard` / `# END project-dashboard` 块包裹，Rust 二进制绝对路径写死，
// `&` fork 后台不阻塞 commit 主进程。
//
// husky / lefthook / pre-commit-framework 冲突检测在 install 前跑，命中即返回 warning。

use std::path::{Path, PathBuf};

pub const HOOK_BEGIN: &str = "# BEGIN project-dashboard";
pub const HOOK_END: &str = "# END project-dashboard";

/// 生成 post-commit hook 里 fleet 块的内容 —— 绝对路径写死，`&` fork 后台。
pub fn hook_block(fleet_binary: &Path, project_id: &str, log_path: &Path) -> String {
    let bin = fleet_binary.to_string_lossy();
    let log = log_path.to_string_lossy();
    format!(
        "{HOOK_BEGIN}\n\"{bin}\" collect --push --project {project_id} 2>>\"{log}\" &\n{HOOK_END}\n"
    )
}

/// 幂等安装：读现有 hook（若无则空），剥离旧的 fleet 块，追加新块，写回。
pub fn install_hook(
    project_path: &Path,
    fleet_binary: &Path,
    project_id: &str,
    log_path: &Path,
) -> std::io::Result<InstalledHook> {
    let hook_path = project_path.join(".git").join("hooks").join("post-commit");
    if let Some(parent) = hook_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&hook_path).unwrap_or_else(|_| String::from("#!/bin/bash\n"));
    let stripped = strip_fleet_block(&existing);
    let block = hook_block(fleet_binary, project_id, log_path);
    let head = if stripped.trim().is_empty() {
        String::from("#!/bin/bash\n")
    } else if stripped.ends_with('\n') {
        stripped
    } else {
        format!("{stripped}\n")
    };
    let content = format!("{head}{block}");
    std::fs::write(&hook_path, &content)?;
    #[cfg(unix)]
    make_executable(&hook_path)?;
    Ok(InstalledHook {
        path: hook_path,
        content,
    })
}

/// 卸载：只剥离 fleet 块，保留用户其他 hook 内容。
pub fn uninstall_hook(project_path: &Path) -> std::io::Result<bool> {
    let hook_path = project_path.join(".git").join("hooks").join("post-commit");
    let existing = match std::fs::read_to_string(&hook_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let stripped = strip_fleet_block(&existing);
    if stripped == existing {
        return Ok(false); // fleet 块本来就不在
    }
    // 剥离后如果只剩 shebang / 空 → 删掉 hook 文件避免污染
    if stripped.trim().is_empty() || stripped.trim() == "#!/bin/bash" {
        std::fs::remove_file(&hook_path)?;
    } else {
        std::fs::write(&hook_path, stripped)?;
    }
    Ok(true)
}

/// 剥离 `# BEGIN project-dashboard` ... `# END project-dashboard` 之间（含边界）的全部行。
pub fn strip_fleet_block(hook_content: &str) -> String {
    let mut out = String::with_capacity(hook_content.len());
    let mut in_block = false;
    for line in hook_content.lines() {
        let t = line.trim();
        if t == HOOK_BEGIN {
            in_block = true;
            continue;
        }
        if t == HOOK_END {
            in_block = false;
            continue;
        }
        if in_block {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 检测 husky / lefthook / pre-commit-framework 之类的 hook 管理框架。
pub fn detect_hook_framework(project_path: &Path) -> Option<HookFramework> {
    if project_path.join(".husky").is_dir() {
        return Some(HookFramework::Husky);
    }
    if project_path.join(".lefthook.yml").is_file()
        || project_path.join(".lefthook.yaml").is_file()
        || project_path.join("lefthook.yml").is_file()
    {
        return Some(HookFramework::Lefthook);
    }
    if project_path.join(".pre-commit-config.yaml").is_file() {
        return Some(HookFramework::PreCommit);
    }
    if let Ok(text) = std::fs::read_to_string(project_path.join("package.json")) {
        if text.contains("\"husky\"") {
            return Some(HookFramework::Husky);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookFramework {
    Husky,
    Lefthook,
    PreCommit,
}

impl HookFramework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Husky => "husky",
            Self::Lefthook => "lefthook",
            Self::PreCommit => "pre-commit-framework",
        }
    }
}

pub struct InstalledHook {
    pub path: PathBuf,
    pub content: String,
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

/// 计划任务 / LaunchAgent 模板 —— 层 2「登录 + 每 6h」触发点。
///
/// Windows：schtasks.exe 用 `/SC ONLOGON` + `/SC HOURLY /MO 6`。
/// macOS：LaunchAgent plist 用 `RunAtLoad` + `StartInterval: 21600`。
pub fn windows_scheduled_task_xml(task_name: &str, fleet_binary: &Path) -> String {
    let bin = fleet_binary.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>project-dashboard 层 2: 登录 + 每 6h 采集推送</Description>
    <URI>\{task_name}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled></LogonTrigger>
    <TimeTrigger>
      <Repetition><Interval>PT6H</Interval></Repetition>
      <StartBoundary>2026-01-01T00:00:00</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>
  </Settings>
  <Actions>
    <Exec>
      <Command>{bin}</Command>
      <Arguments>collect --push --all</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

pub fn mac_launchagent_plist(label: &str, fleet_binary: &Path) -> String {
    let bin = fleet_binary.to_string_lossy();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>collect</string>
        <string>--push</string>
        <string>--all</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StartInterval</key>
    <integer>21600</integer>
</dict>
</plist>
"#
    )
}
