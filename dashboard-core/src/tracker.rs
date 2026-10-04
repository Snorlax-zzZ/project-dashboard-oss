// M2 §4 采集器 —— shell out `git` 采集项目状态字段。
// v4.1 定死不引 `git2` crate。
// 字段集固定：name / project_id / branch / head / upstream / ahead / behind / dirty / untracked /
//              last_commit_at / last_fs_change_at / collected_at / progress / dangerous_state。
//
// v4.2（Decision 15）: UUID 权威从项目根 `.dashboard-project-id` 迁到 `dashboard-state/registry/`。
// 采集主键由 caller 从 registry iterator 传入并写回 status.project_id；tracker 本身不再读写文件。
// `get_git_remote_url` 仅供 UI 层新增/关联判断 + sync:true 可选防伪校验用，不用于采集主键读取。

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectStatus {
    pub project_id: Option<Uuid>,
    pub name: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// v4.3 新增：HEAD 完整 40 位 sha。用于跨机严格相等比对（短 sha 长度可能因两机对象库不同而差异）
    /// 和 head/log 原子性校验。None = 老 v4.2 snapshot / 有效空仓 / rev-parse HEAD 命令失败。
    pub head_full: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub dirty: u32,
    pub untracked: u32,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub last_fs_change_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub progress: Option<Progress>,
    pub dangerous_state: Option<DangerousState>,
    /// v4.3 新增：`git log -n 20` 最近提交历史，**三态语义**。
    /// - `None`：老 v4.2 snapshot / 采集失败（不可用信号）
    /// - `Some(vec![])`：有效空仓（unborn repo，`.git` 存在但无 commit）
    /// - `Some(非空)`：有效证据
    /// git 命令失败**绝不**降级成 `Some(vec![])`，避免误导用户以为对方机器上真没提交。
    pub recent_commits: Option<Vec<CommitEntry>>,
}

/// v4.3+ 新增：单条 commit 的文件级改动（`git show --numstat` 一行）。
/// 二进制文件 numstat 行数输出 `-`，此时 additions/deletions 记为 None。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: String,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

/// v4.3 新增：单条 commit 元数据，构成 `ProjectStatus::recent_commits` 里每一项。
/// - `sha_full` 存完整 40 位 sha（UI 展示时缩写到 7 位）
/// - `subject` 采集时截断到 40 字，隐私边界收窄
/// - `files` 只前 5 条带（`git show --numstat` 文件级增删，用于看清两端分支改了什么）；
///   其余条目 / 老 snapshot / 采集失败 → None。只采前 5 条避免放大 snapshot、拖累跨机同步。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommitEntry {
    pub sha_full: String,
    pub subject: String,
    pub author: String,
    pub time: DateTime<Utc>,
    #[serde(default)]
    pub files: Option<Vec<FileChange>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    pub next: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DangerousState {
    Rebase,
    Merge,
    #[serde(rename = "cherry-pick")]
    CherryPick,
    Detached,
}

impl DangerousState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Detached => "detached",
        }
    }
}

/// 只读采集 —— 只跑只读 git 命令（`git status / log / rev-parse / rev-list / symbolic-ref`），
/// 绝不 fetch / 不切分支 / 不 stash。
pub fn collect(project_path: &Path, name: &str) -> ProjectStatus {
    let now = Utc::now();
    // v4.2: project_id 由 caller 从 registry iterator 传入并写回；此处置 None。
    let project_id: Option<Uuid> = None;
    let dangerous_state = detect_dangerous_state(project_path);
    let progress = read_progress(project_path);

    let branch = if dangerous_state == Some(DangerousState::Detached) {
        None
    } else {
        run_git_opt(project_path, &["symbolic-ref", "--short", "HEAD"])
    };
    let upstream = run_git_opt(project_path, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
    let (ahead, behind) = if upstream.is_some() {
        let ahead = run_git_opt(project_path, &["rev-list", "--count", "@{upstream}..HEAD"])
            .and_then(|s| s.parse().ok());
        let behind = run_git_opt(project_path, &["rev-list", "--count", "HEAD..@{upstream}"])
            .and_then(|s| s.parse().ok());
        (ahead, behind)
    } else {
        (None, None)
    };
    let (dirty, untracked) = count_status(project_path);
    let last_commit_at = run_git_opt(project_path, &["log", "-1", "--format=%cI"])
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let last_fs_change_at = project_path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| Utc.timestamp_opt(d.as_secs() as i64, d.subsec_nanos()).single());

    // v4.3 §1.2：一次调用同时采 head_full + recent_commits，含三态语义 + head/log 原子性校验。
    let (head_full, recent_commits) = collect_head_and_recent_commits(project_path);
    // v4.3+：给最近 5 条补文件级改动（git show --numstat），供 UI 看清两端分支各改了什么。
    let recent_commits = enrich_commits_with_files(project_path, recent_commits, 5);
    // P0-2：短 head 从 head_full 前 7 位派生，不再独立跑 rev-parse --short HEAD，
    // 消除两次 rev-parse 之间的时序间隙（间隙内若发生 commit，head 与 head_full 会指向不同提交）。
    let head = head_full.as_ref().map(|h| h.chars().take(7).collect::<String>());

    ProjectStatus {
        project_id,
        name: name.to_string(),
        branch,
        head,
        head_full,
        upstream,
        ahead,
        behind,
        dirty,
        untracked,
        last_commit_at,
        last_fs_change_at,
        collected_at: now,
        progress,
        dangerous_state,
        recent_commits,
    }
}

/// 读项目根 `git remote get-url origin` 并 trim 尾换行。
///
/// v4.2（Decision 15）用途仅限：
/// - UI 层新增/关联判断（前端调 `find_project_by_git_url` 反查 registry）
/// - sync:true 采集时可选防伪校验（对比 registry 里该 UUID 的 `git_url` / `git_url_aliases`）
///
/// **不用于采集主键读取** —— 采集主键由 caller 从 registry iterator 传入。
///
/// 返回 None 的情形：非 git 仓 / `origin` remote 未配置 / `git remote get-url` 非 0 退出 /
/// stdout 为空。
pub fn get_git_remote_url(project_path: &Path) -> Option<String> {
    run_git_opt(project_path, &["remote", "get-url", "origin"])
}

/// 从项目根 `.dashboard-progress.yaml` 读 `{done, total, next}`；文件无/格式非法 → None（**不推断**）。
pub fn read_progress(project_path: &Path) -> Option<Progress> {
    let text = std::fs::read_to_string(project_path.join(".dashboard-progress.yaml")).ok()?;
    serde_yaml::from_str(&text).ok()
}

/// 识别 rebase / merge / cherry-pick / detached。
///
/// 审计对账 P0-F（2026-07-06）：`.git` 在 worktree / submodule 场景下
/// 是**文件**而不是目录，内容形如 `gitdir: /path/to/real/gitdir`。原实现仅用
/// `git_dir.join(...).exists()` 判断，worktree 中 rebase/merge 状态会被漏报，
/// 用户可能在 rebase 中误推。修复：`.git` 是 file 时解析 gitdir: 内容拿真实路径。
pub fn detect_dangerous_state(project_path: &Path) -> Option<DangerousState> {
    let real_git_dir = resolve_git_dir(project_path)?;
    if real_git_dir.join("rebase-merge").exists() || real_git_dir.join("rebase-apply").exists() {
        return Some(DangerousState::Rebase);
    }
    if real_git_dir.join("MERGE_HEAD").exists() {
        return Some(DangerousState::Merge);
    }
    if real_git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some(DangerousState::CherryPick);
    }
    // Detached HEAD: `git symbolic-ref --quiet HEAD` 退出码非零
    let out = Command::new("git")
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(DangerousState::Detached);
    }
    None
}

/// 解析项目根下 `.git` 的**真实 git 目录**：
///   - 是目录 → 直接返回
///   - 是 file → 读内容取 `gitdir: <path>`，返回该路径（支持绝对 / 相对）
///   - 不存在 → None
pub fn resolve_git_dir(project_path: &Path) -> Option<PathBuf> {
    let dot_git = project_path.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        // 内容形如 `gitdir: ../.git/worktrees/foo\n`
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with("gitdir:"))?;
        let p = line.trim_start().trim_start_matches("gitdir:").trim();
        if p.is_empty() {
            return None;
        }
        let candidate = PathBuf::from(p);
        if candidate.is_absolute() {
            Some(candidate)
        } else {
            Some(project_path.join(candidate))
        }
    } else {
        None
    }
}

/// git_url 归一化（放开准入 + 收紧格式契约）。
///
/// 放开：移除 host 白名单（任意合法 host 接受）、接受 http、路径放开多段（≥2 段）。
/// 收紧：禁 userinfo/query/fragment/空段、每段仅 `[A-Za-z0-9._-]`、host 单独校验、端口支持。
/// git_url 只作跨机身份指纹与防伪校验，App 从不用它 clone/fetch，故放开 host 不引入网络风险。
///
/// 步骤（顺序固定）：协议统一 https → 拆 host[:port] 与 path → path 去首尾斜杠 →
/// 大小写无关去尾段 `.git` → 按 `/` 切段每段小写 → host 小写单校 → 格式契约 → 拼输出。
/// 任一契约违反返回 `NormalizeError::Malformed`（不产出脏 canonical，覆盖 AC-5）。
///
/// 大小写：本轮统一小写输出（存量 canonical 免迁移）。自建大小写敏感仓（`Repo` ≠ `repo`）
/// 需手动保持书写一致，本轮不支持大小写敏感身份（保留大小写单独版本化迁移再做）。
pub fn normalize_git_url(url: &str) -> Result<String, NormalizeError> {
    let url = url.trim();
    let malformed = || NormalizeError::Malformed(url.to_string());

    // 1. 协议归一：http/https/git@ 三种书写形式统一，拆出 host[:port] 段与 path 段。
    //    SSH 用 `:` 分隔（git@host:owner/repo），HTTP(S) 用第一个 `/` 分隔。
    let (host_port, path) = if let Some(rest) = url.strip_prefix("git@") {
        rest.split_once(':').ok_or_else(malformed)?
    } else if let Some(rest) = url.strip_prefix("https://") {
        rest.split_once('/').ok_or_else(malformed)?
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest.split_once('/').ok_or_else(malformed)?
    } else {
        return Err(malformed());
    };

    // 2. 格式契约：禁 userinfo（`@`，git@ 前缀已在上一步消费，剩余 `@` 即 userinfo）、
    //    禁 query（`?`）、禁 fragment（`#`）。防 `https://user:token@host` 把凭证写进身份键与日志。
    for seg in [host_port, path] {
        if seg.contains('@') || seg.contains('?') || seg.contains('#') {
            return Err(malformed());
        }
    }

    // 3. host 单独校验（拆可选 :port）。host 仅允许 [a-z0-9.-]，port 仅数字；
    //    IPv6 字面量（含多个 `:`）本轮不支持，落 Malformed。
    let host_lc = host_port.to_lowercase();
    let (host, port) = match host_lc.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host_lc.as_str(), None),
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    if !host_ok {
        return Err(malformed());
    }
    if let Some(p) = port {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(malformed());
        }
    }

    // 4. path 处理（顺序固定）：a. 去首尾斜杠 b. 大小写无关去尾段 `.git` c. 按 `/` 切段每段小写。
    //    去 `.git` 用字节比较（`.git` 全 ASCII，命中时 len-4 必是 char 边界，非 ASCII path 不会 panic）。
    let path_trimmed = path.trim_matches('/');
    let bytes = path_trimmed.as_bytes();
    let path_no_git = if bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".git") {
        &path_trimmed[..path_trimmed.len() - 4]
    } else {
        path_trimmed
    };
    let segs: Vec<String> = path_no_git.split('/').map(|s| s.to_lowercase()).collect();

    // 5. 段校验：段数 ≥ 2（与 schema `{2,}` 对齐）、每段非空且仅 [a-z0-9._-]（已小写）。
    if segs.len() < 2 {
        return Err(malformed());
    }
    for s in &segs {
        let seg_ok = !s.is_empty()
            && s.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
            });
        if !seg_ok {
            return Err(malformed());
        }
    }

    // 6. 拼输出：https://{host}[:{port}]/{seg1}/.../{segN} 全小写。
    let host_out = match port {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    Ok(format!("https://{}/{}", host_out, segs.join("/")))
}

/// URL 绑定分类结果（Q1-C）：本地真实 origin 归一化值与用户手填归一化值的关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlBinding {
    /// origin 与手填逐字节一致 → 直接过，primary = origin。
    Consistent,
    /// Bitbucket web/clone 双形态（结构性等价但书写不同）→ 需用户显式确认后过。
    BitbucketDualForm,
    /// 完全无关（既非一致也非双形态）→ 拒绝保存（防选错项目目录）。
    Unrelated,
    /// 本地无 origin（新 git init 未加 remote）→ 回退用手填作 primary。
    NoOrigin,
}

/// 分类 origin 归一化值与手填归一化值的关系（两者都应是 `normalize_git_url` 产出）。
///
/// **纯函数只做分类**：primary / alias / actual_origin / 错误消息拼装全部由调用方
/// `add_project` 决定，IO（读 origin）与推导都留调用方（便于单测 + 边界清晰）。
pub fn classify_url_binding(origin_norm: Option<&str>, input_norm: &str) -> UrlBinding {
    match origin_norm {
        None => UrlBinding::NoOrigin,
        Some(o) if o == input_norm => UrlBinding::Consistent,
        Some(o) if is_bitbucket_dual_form(o, input_norm) => UrlBinding::BitbucketDualForm,
        Some(_) => UrlBinding::Unrelated,
    }
}

/// 判断两个已归一化 URL 是否为 Bitbucket web/clone 双形态（同一仓不同书写）。
/// web:   `https://host/projects/<owner>/repos/<repo>`
/// clone: `https://host/scm/<owner>/<repo>`
/// 判定两者解析出相同 `(host, owner, repo)` 身份。**只识别这一对已知形态，不做通用 URL 规则引擎**
/// （两方审计一致反对）；解析失败即判非双形态。
fn is_bitbucket_dual_form(a: &str, b: &str) -> bool {
    match (parse_bitbucket_identity(a), parse_bitbucket_identity(b)) {
        (Some(ia), Some(ib)) => ia == ib,
        _ => false,
    }
}

/// 从归一化 URL 解析 Bitbucket `(host, owner, repo)` 身份，支持 web/clone 两形态；否则 None。
fn parse_bitbucket_identity(url: &str) -> Option<(&str, &str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let segs: Vec<&str> = path.split('/').collect();
    match segs.as_slice() {
        ["projects", owner, "repos", repo] => Some((host, owner, repo)),
        ["scm", owner, repo] => Some((host, owner, repo)),
        _ => None,
    }
}

fn run_git_opt(project_path: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(project_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// v4.3 §1.2.4：跑 `git log -n <limit> --pretty=format:"%H%x1f%s%x1f%an%x1f%aI"` 解析成
/// `Vec<CommitEntry>`。
///
/// - subject 采集时截断到 40 chars（隐私边界收窄；chars 而不是 bytes，中文 subject 安全）
/// - 命令失败 / 输出行格式无效 / sha 长度非 40 / 时间戳解析失败 → 返回 `None`
///   （三态"不可用"信号；**绝不**降级成 `Some(vec![])`）
/// - 命令成功但 stdout 为空 → 返回 `Some(vec![])`（有效"空 log"，通常发生在极端 shallow clone 场景；
///   unborn repo 由 `collect_head_and_recent_commits` 上一层通过 `--is-inside-work-tree` +
///   rev-parse HEAD 失败判定处理，不会走到这里）
///
/// %x1f = US（Unit Separator，ASCII 0x1f），选它是因为几乎不可能出现在 subject / author 里。
pub(crate) fn run_git_log_recent(project_path: &Path, limit: u32) -> Option<Vec<CommitEntry>> {
    let limit_arg = format!("-n{limit}");
    let pretty_arg = "--pretty=format:%H%x1f%s%x1f%an%x1f%aI";
    let out = Command::new("git")
        .args(["log", &limit_arg, pretty_arg])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\x1f');
        let sha = parts.next()?;
        let subject = parts.next()?;
        let author = parts.next()?;
        let time_str = parts.next()?;
        if parts.next().is_some() {
            // 意外多字段（subject/author 里混入 US？）—— 视作解析失败
            return None;
        }
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let time = DateTime::parse_from_rfc3339(time_str.trim())
            .ok()?
            .with_timezone(&Utc);
        let subject_trunc: String = subject.chars().take(40).collect();
        commits.push(CommitEntry {
            sha_full: sha.to_string(),
            subject: subject_trunc,
            author: author.to_string(),
            time,
            files: None, // 由 enrich_commits_with_files 给前 5 条补
        });
    }
    Some(commits)
}

/// 解析 `git show --numstat --format=` 输出：每行 `<additions>\t<deletions>\t<path>`。
/// 二进制文件行数为 `-` → None。少字段 / 空行跳过。抽成纯函数便于单测。
fn parse_numstat(text: &str) -> Vec<FileChange> {
    let mut files = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        // splitn(3)：path 保留剩余部分（含可能的 tab，如 rename 的 `a => b`）
        let mut parts = line.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        files.push(FileChange {
            path: path.to_string(),
            additions: add.parse::<u32>().ok(), // "-"（二进制）→ None
            deletions: del.parse::<u32>().ok(),
        });
    }
    files
}

/// 跑 `git show --numstat --format= <sha>` 拿单条 commit 的文件级增删。
/// 命令失败 → None（不可用信号）；成功但无输出（如空 commit）→ Some(vec![])。
fn collect_commit_files(project_path: &Path, sha: &str) -> Option<Vec<FileChange>> {
    let out = Command::new("git")
        .args(["show", "--numstat", "--format=", sha])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_numstat(&String::from_utf8_lossy(&out.stdout)))
}

/// 给前 n 条 commit 补文件级改动（`git show --numstat`）。只前 n 条：满足"看清近几次改了
/// 什么"又不放大 snapshot。None（无有效 commit）原样返回，空 vec 不触发 git 调用。
fn enrich_commits_with_files(
    project_path: &Path,
    commits: Option<Vec<CommitEntry>>,
    n: usize,
) -> Option<Vec<CommitEntry>> {
    commits.map(|mut cs| {
        for c in cs.iter_mut().take(n) {
            c.files = collect_commit_files(project_path, &c.sha_full);
        }
        cs
    })
}

/// 内部：原子采一次 `git rev-parse HEAD` + `git log -n <limit>`。
/// 语义参见 [`collect_head_and_recent_commits`]。
fn atomic_head_and_log(
    project_path: &Path,
    limit: u32,
) -> (Option<String>, Option<Vec<CommitEntry>>) {
    let head_full = run_git_opt(project_path, &["rev-parse", "HEAD"]);
    let commits = run_git_log_recent(project_path, limit);
    (head_full, commits)
}

/// v4.3 §1.2.5：一次采集 head_full + recent_commits，含三态语义 + head/log 原子性校验。
///
/// 返回 tuple 语义：
/// - `(None, None)` —— 非 git 仓 / git binary 缺失 / 目录不在 work tree 内，**或**已在 work
///   tree 内但 HEAD 采集失败（loose ref / packed-refs 损坏、symref 环等，P0-1）（**不可用信号**）
/// - `(None, Some(vec![]))` —— 有效空仓 unborn repo（`.git` 存在但无 commit）
/// - `(Some(head_full), Some(commits))` —— 有效证据；且 `commits.first().sha_full == head_full`
/// - `(Some(head_full), None)` —— head_full 采到但 log 采集失败 / head 与 log 反复不一致
///
/// **契约不变式**：绝不返回 `(None, Some(非空))` 或 `(Some(_), Some(vec![]))`（矛盾状态）。
///
/// **原子性**：先跑 rev-parse HEAD 拿 head_full → 再跑 git log 拿 recent_commits → 校验
/// `first().sha_full == head_full`。不一致（采集期间发生 commit）→ **重新采一次完整的
/// rev-parse + log 组合**；仍不一致 → `recent_commits = None`（不给错数据）。
///
/// 只做一次重试的原因：连续两次都撞正在 commit 是低概率，标 None 让 aggregate 走
/// Insufficient 分支比"给错数据"安全。
pub(crate) fn collect_head_and_recent_commits(
    project_path: &Path,
) -> (Option<String>, Option<Vec<CommitEntry>>) {
    // 第一步：先判断是否 git 仓（`--is-inside-work-tree` 输出 `true` 才算）—— 覆盖
    // "非 git 仓" 和 "git binary 缺失 / 目录不存在" 场景。
    let is_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_path)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false);
    if !is_repo {
        return (None, None);
    }

    // 第二步：原子采一次 head + log
    let (h1, l1) = atomic_head_and_log(project_path, 20);
    match (h1, l1) {
        // HEAD + log 都采到 → 校验一致性（不一致 = 采集期间发生 commit，重采一次）
        (Some(head_full), Some(commits)) => {
            if commits.first().map(|c| c.sha_full.as_str()) == Some(head_full.as_str()) {
                return (Some(head_full), Some(commits));
            }
            reacquire_or_none(project_path, Some(head_full))
        }
        // HEAD 采到但 log 失败 → 保 head_full，recent_commits = None（不可用信号）
        (Some(head_full), None) => (Some(head_full), None),
        // P0-1：HEAD 未采到但 log 非空 = 矛盾状态（首次 commit 竞态：采 head
        // 与采 log 之间刚发生首次 commit）。重采一次；仍矛盾 → (None,None)，绝不把非空 log
        // 丢掉报成空仓。
        (None, Some(commits)) if !commits.is_empty() => reacquire_or_none(project_path, None),
        // P0-1：HEAD 未采到 + log 空/失败 → 用 symbolic-ref 区分 unborn vs
        // 采集失败。symbolic-ref --quiet HEAD 成功 = 指向未创建分支的 unborn 空仓 →
        // (None, Some(vec![]))；失败（实测 exit 128：loose ref / packed-refs 损坏、symref 环）
        // = 采集失败 → (None,None)，不违反"git 失败绝不降级空数组"契约。
        (None, _) => {
            if head_symbolic_ref_ok(project_path) {
                (None, Some(Vec::new()))
            } else {
                (None, None)
            }
        }
    }
}

/// P0-1：`git symbolic-ref --quiet HEAD` 成功 → HEAD 是指向"尚未创建分支"的符号引用 =
/// 有效 unborn 空仓；失败（实测 exit 128）→ loose ref / packed-refs 损坏、symref 自环等 =
/// 采集失败，不可降级成空仓。
///
/// 实测依据：unborn（git init 无 commit）symbolic-ref 返回 0；损坏 `.git/refs/heads/<branch>`
/// 或 symref 环时 symbolic-ref 返回 128。据此区分 `(None, Some(vec![]))` 与 `(None, None)`。
fn head_symbolic_ref_ok(project_path: &Path) -> bool {
    Command::new("git")
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .current_dir(project_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// P0-1：head/log 首采不一致或矛盾时重采一次，校验 `first().sha_full == head_full` 一致才返回
/// 新证据；否则返回 `(fallback_head, None)`（把已确证的 head_full 传进来保留，无则 None）。
fn reacquire_or_none(
    project_path: &Path,
    fallback_head: Option<String>,
) -> (Option<String>, Option<Vec<CommitEntry>>) {
    let (h2, l2) = atomic_head_and_log(project_path, 20);
    match (h2, l2) {
        (Some(hf2), Some(c2)) if c2.first().map(|c| c.sha_full.as_str()) == Some(hf2.as_str()) => {
            (Some(hf2), Some(c2))
        }
        (h2, _) => (h2.or(fallback_head), None),
    }
}

fn count_status(project_path: &Path) -> (u32, u32) {
    let out = match Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(project_path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return (0, 0),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut dirty = 0u32;
    let mut untracked = 0u32;
    for line in text.lines() {
        if line.starts_with("??") {
            untracked += 1;
        } else if !line.trim().is_empty() {
            dirty += 1;
        }
    }
    (dirty, untracked)
}

/// 项目根检测辅助：判断是否为 git 仓（含 `.git` 目录 or worktree/submodule 的 `.git` 文件）。
pub fn is_git_repo(project_path: &Path) -> bool {
    resolve_git_dir(project_path).is_some()
}

/// 备胎命令入口：emergency-purge 需要的前置依赖检测。
/// 检测 `git-filter-repo` 是否安装（`git filter-repo --version` 或 `python3 -m git_filter_repo --version`）。
pub fn detect_git_filter_repo() -> bool {
    Command::new("git")
        .args(["filter-repo", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        || Command::new("python3")
            .args(["-m", "git_filter_repo", "--version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeError {
    Malformed(String),
}

impl std::fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(u) => write!(f, "git_url 格式非法: {u}"),
        }
    }
}

impl std::error::Error for NormalizeError {}

#[derive(Debug)]
pub enum TrackerError {
    Io(std::io::Error),
    NotAGitRepo(PathBuf),
    GitCommandFailed { args: Vec<String>, stderr: String },
}

impl std::fmt::Display for TrackerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::NotAGitRepo(p) => write!(f, "not a git repo: {}", p.display()),
            Self::GitCommandFailed { args, stderr } => {
                write!(f, "git {} failed: {}", args.join(" "), stderr)
            }
        }
    }
}

impl std::error::Error for TrackerError {}

impl From<std::io::Error> for TrackerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // numstat 解析：正常增删行数
    #[test]
    fn parse_numstat_basic() {
        let got = parse_numstat("12\t3\tsrc/main.rs\n0\t5\tREADME.md\n");
        assert_eq!(
            got,
            vec![
                FileChange {
                    path: "src/main.rs".to_string(),
                    additions: Some(12),
                    deletions: Some(3),
                },
                FileChange {
                    path: "README.md".to_string(),
                    additions: Some(0),
                    deletions: Some(5),
                },
            ]
        );
    }

    // 二进制文件 numstat 输出 "-" → additions/deletions None
    #[test]
    fn parse_numstat_binary_dash_to_none() {
        let got = parse_numstat("-\t-\tassets/logo.png\n");
        assert_eq!(
            got,
            vec![FileChange {
                path: "assets/logo.png".to_string(),
                additions: None,
                deletions: None,
            }]
        );
    }

    // 少字段 / 空行跳过；含空格路径保留
    #[test]
    fn parse_numstat_skips_malformed_and_keeps_spaced_path() {
        let got = parse_numstat("garbage\n\n7\t1\tdir/a b.rs\n");
        assert_eq!(
            got,
            vec![FileChange {
                path: "dir/a b.rs".to_string(),
                additions: Some(7),
                deletions: Some(1),
            }]
        );
    }

    #[test]
    fn normalize_https_lowercases_and_strips_git_suffix() {
        assert_eq!(
            normalize_git_url("https://GitHub.com/example/Project-Dashboard.git").unwrap(),
            "https://github.com/example/project-dashboard"
        );
    }

    #[test]
    fn normalize_ssh_to_https() {
        assert_eq!(
            normalize_git_url("git@github.com:example/project-dashboard.git").unwrap(),
            "https://github.com/example/project-dashboard"
        );
    }

    #[test]
    fn normalize_accepts_self_hosted_gitea() {
        assert_eq!(
            normalize_git_url("https://gitea.company.internal/team/proj.git").unwrap(),
            "https://gitea.company.internal/team/proj"
        );
    }

    #[test]
    fn normalize_rejects_ssh_alias() {
        let err = normalize_git_url("gh-work:owner/repo").unwrap_err();
        assert!(matches!(err, NormalizeError::Malformed(_)));
    }

    #[test]
    fn normalize_accepts_any_host() {
        // 放开 host 白名单：任意合法 host 接受（git_url 只作跨机身份指纹，App 从不用它 clone，无网络风险）
        assert_eq!(
            normalize_git_url("https://random-host.example/o/r").unwrap(),
            "https://random-host.example/o/r"
        );
    }

    #[test]
    fn normalize_rejects_missing_repo() {
        let err = normalize_git_url("https://github.com/onlyowner").unwrap_err();
        assert!(matches!(err, NormalizeError::Malformed(_)));
    }

    #[test]
    fn normalize_accepts_multi_segment() {
        // 放开路径两段强制：多段（如 Bitbucket /projects/X/repos/Y 四段）保留
        assert_eq!(
            normalize_git_url("https://host.example/projects/x/repos/y").unwrap(),
            "https://host.example/projects/x/repos/y"
        );
    }

    #[test]
    fn normalize_accepts_http_scheme() {
        // 接受 http，协议归一到 https
        assert_eq!(
            normalize_git_url("http://host.example/o/r").unwrap(),
            "https://host.example/o/r"
        );
    }

    #[test]
    fn normalize_accepts_company_bitbucket() {
        // 自建 Bitbucket：http + 自建 host + 四段路径（旧版三重不满足），现全接受并小写（AC-1）
        assert_eq!(
            normalize_git_url(
                "http://git.example.org/projects/TEAM/repos/ExampleApp"
            )
            .unwrap(),
            "https://git.example.org/projects/team/repos/exampleapp"
        );
    }

    #[test]
    fn normalize_accepts_port() {
        // 自建仓常带非标端口，保留 :port
        assert_eq!(
            normalize_git_url("https://host.example:7990/o/r").unwrap(),
            "https://host.example:7990/o/r"
        );
    }

    #[test]
    fn normalize_git_suffix_order() {
        // 顺序固定：先去尾斜杠再大小写无关去 .git（AC-3 幂等前提）
        assert_eq!(
            normalize_git_url("https://host.example/O/R.git/").unwrap(),
            "https://host.example/o/r"
        );
    }

    #[test]
    fn normalize_ssh_suffix_order() {
        // SSH 形式尾斜杠 / .git 处理与 HTTPS 一致
        assert_eq!(
            normalize_git_url("git@host.example:O/R.git/").unwrap(),
            "https://host.example/o/r"
        );
    }

    #[test]
    fn normalize_rejects_userinfo() {
        // 禁 userinfo，防 token 泄露进身份键 / 错误日志（AC-5）
        assert!(matches!(
            normalize_git_url("https://user:token@host.example/o/r").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
    }

    #[test]
    fn normalize_rejects_query_fragment() {
        assert!(matches!(
            normalize_git_url("https://host.example/o/r?x=1").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
        assert!(matches!(
            normalize_git_url("https://host.example/o/r#frag").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
    }

    #[test]
    fn normalize_rejects_empty_segment() {
        // 连续 // / 空段（AC-5）
        assert!(matches!(
            normalize_git_url("https://host.example/o//r").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
    }

    #[test]
    fn normalize_rejects_illegal_char() {
        // 段内非 [A-Za-z0-9._-] 字符（AC-5）
        assert!(matches!(
            normalize_git_url("https://host.example/o/r epo").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
    }

    #[test]
    fn normalize_rejects_single_segment() {
        // 段数 < 2 → Malformed（与 schema {2,} 对齐）
        assert!(matches!(
            normalize_git_url("https://host.example/only").unwrap_err(),
            NormalizeError::Malformed(_)
        ));
    }

    // ---- Q1-C：classify_url_binding 四路 ----

    #[test]
    fn classify_binding_consistent() {
        assert_eq!(
            classify_url_binding(Some("https://host.example/o/r"), "https://host.example/o/r"),
            UrlBinding::Consistent
        );
    }

    #[test]
    fn classify_binding_no_origin() {
        assert_eq!(
            classify_url_binding(None, "https://host.example/o/r"),
            UrlBinding::NoOrigin
        );
    }

    #[test]
    fn classify_binding_bitbucket_dual_form_both_directions() {
        let web = "https://h.example/projects/team/repos/exampleapp";
        let clone = "https://h.example/scm/team/exampleapp";
        // origin=clone（真实 origin 常是 clone 形式）、input=web（用户手填 web）
        assert_eq!(
            classify_url_binding(Some(clone), web),
            UrlBinding::BitbucketDualForm
        );
        // 反向：origin=web、input=clone
        assert_eq!(
            classify_url_binding(Some(web), clone),
            UrlBinding::BitbucketDualForm
        );
    }

    #[test]
    fn classify_binding_unrelated() {
        // 归一化毫不相关
        assert_eq!(
            classify_url_binding(Some("https://h.example/a/b"), "https://h.example/c/d"),
            UrlBinding::Unrelated
        );
        // 不同 host 的 Bitbucket 双形态：host 不同 → identity 不等 → 无关（不误判为等价）
        assert_eq!(
            classify_url_binding(
                Some("https://h1.example/scm/x/y"),
                "https://h2.example/projects/x/repos/y"
            ),
            UrlBinding::Unrelated
        );
    }

    #[test]
    fn classify_binding_different_port_is_unrelated() {
        // Q1-C：host 同名但端口不同 → identity(含端口) 不等 → 无关，不误合并成双形态
        assert_eq!(
            classify_url_binding(
                Some("https://h.example:7990/scm/x/y"),
                "https://h.example:8443/projects/x/repos/y"
            ),
            UrlBinding::Unrelated
        );
    }

    #[test]
    fn dangerous_state_serialized_as_kebab_case() {
        let s = serde_json::to_string(&DangerousState::CherryPick).unwrap();
        assert_eq!(s, "\"cherry-pick\"");
    }
}
