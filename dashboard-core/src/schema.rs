// Schema Allowlist validator（M2 §3.6）。
// 明文 include_str! 三份 schema.json，任何 dashboard-state 落盘前必须过一遍：
//   - `additionalProperties:false` 拒收未声明字段
//   - 敏感字段名黑名单（diff/hunk/patch/prompt/message/response/chat/session_id/files_content/file_content）
// 兜底扫描：即便 schema 白名单允许，字段名或字符串内容包含黑名单关键词也拒收。

use jsonschema::Validator;
use std::sync::OnceLock;

const REGISTRY_PROJECT_SCHEMA: &str =
    include_str!("../../schemas/registry-project.schema.json");
const SNAPSHOT_SCHEMA: &str = include_str!("../../schemas/snapshot.schema.json");
const EVENTS_SCHEMA: &str = include_str!("../../schemas/events.schema.json");

// 敏感**字段名**黑名单（走 `contains`，字段名误伤面小）
// 审计对账 P0-E（2026-07-06）：扩展关键词覆盖度 + 归一化命名。
// **契约不变式**：所有关键词必须写成归一化形态（无 `_/-`，全小写），因为 `normalize_ident`
// 会去掉分隔符后再 `contains`。带 `_` 的关键词永远不会命中。
const SENSITIVE_KEYWORDS: &[&str] = &[
    "diff",
    "hunk",
    "patch",
    "prompt",
    "response",
    "chat",
    "sessionid",
    "filescontent",
    "filecontent",
    // 扩展：常见密钥 / 认证关键词（P1-3）
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "bearer",
    "authorization",
    "credential",
];

/// 值层敏感关键词 —— 比字段名黑名单更保守，只收几个高置信度的
/// **敏感字段值命中片段**，避免 project name 含 "token"/"auth" 就被拒。
///
/// P1-C（2026-07-06）：value 层用完整 `SENSITIVE_KEYWORDS`
/// 会把 `auth-token-service`、`jwt-rotation` 类合法项目名一并拒收。
///
/// 二次收窄 P1-F（2026-07-07）：`patch/chat/response` 通过 `contains`
/// 会把 `dispatch-service`（含 patch）、`chatbot-ui`（含 chat）、`incident-response`（含 response）
/// 等业务名一并拒收。这三个词的**字段名层**保护仍在 `SENSITIVE_KEYWORDS`，不需要值层重复拦截。
///
/// 最终 value 层只保留几乎不可能作为业务名 substring 出现的：
/// - `filescontent` / `filecontent` / `sessionid` —— 无 - / _ 分隔，业务名不会撞
/// - `prompt` —— 极少作为业务名 substring（如"prompt-engineering"是少数场景，权衡下继续拦）
/// - `diff` / `hunk` —— git 语义强绑定，业务名极少含
///
/// 认证类关键词（`token`/`secret`/`password`/`apikey`/`bearer`/`authorization`/`credential`）
/// 和会话/内容类（`patch`/`chat`/`response`/`message`）**只在字段名**层拦截。
/// 字段值层依靠 `SECRET_PREFIXES` 高熵前缀边界匹配作为兜底。
const SENSITIVE_VALUE_KEYWORDS: &[&str] = &[
    "filescontent",
    "filecontent",
    "prompt",
    "sessionid",
    "diff",
    "hunk",
];
// message 单独列，因为 `machines` 里可能有 "machine" 前缀撞词 —— 这里只作用于字段名精确匹配
const SENSITIVE_EXACT_FIELD_NAMES: &[&str] = &["message"];

/// 高熵密钥前缀（Anthropic / GitHub / Slack / AWS 常见形态）—— 都是唯一性前缀，
/// 特意排除 `sk-`（过短会误伤 `task-*` / `risk-*` / `asterisk-*` 之类正常字符串）。
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-", // Anthropic
    "ghp_",    // GitHub personal
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "xoxb-", // Slack bot
    "xoxp-",
    "xoxa-",
    "AKIA", // AWS Access Key ID
    "ASIA", // AWS Session
];

/// 判断字符串值是否**在 token 边界处**含足够长的高熵密钥前缀。
///
/// P1-C（2026-07-06）：原 `contains` 语义会把
/// `"api.xoxa-endpoint.com"` 里的 `xoxa-` 也当作密钥拒收（合法域名 → false positive）。
///
/// 收窄规则：字符串沿 whitespace/punctuation 切成 token（`_/-` 视为 token 内字符），
/// 某个 token **以 prefix 起始** 且 **prefix 后至少还有 MIN_SUFFIX 个字符** 才算命中。
///
/// 阈值 MIN_SUFFIX=12 的动机：真实密钥/token 都很长（sk-ant-api03-XXX / ghp_XXX 40字符 /
/// xoxb-1234567890-abcdefghij / AKIA... 20 字符）。合法域名 `xoxa-endpoint`（后缀 8 字符）
/// 不会命中；`user-xoxa-service` 因整个 token 不以 `xoxa-` 起始也不命中。
///
/// - `"sk-ant-api03-abcdef"` (7+12=19 字符 token) → ✓ 命中
/// - `"leaked: ghp_abcdef1234567890"` (split 后 token `ghp_abcdef1234567890` 4+16) → ✓ 命中
/// - `"AKIAIOSFODNN7EXAMPLE"` (4+16) → ✓ 命中
/// - `"api.xoxa-endpoint.com"` (token `xoxa-endpoint` 5+8, 后缀 <12) → ✗ 不命中
/// - `"user-xoxa-service"` (整 token 以 `user` 起始) → ✗ 不命中
fn string_value_matches_secret_prefix(s_lc: &str, prefix_lc: &str) -> bool {
    const MIN_SUFFIX: usize = 12;
    let min_tok_len = prefix_lc.len() + MIN_SUFFIX;
    for tok in s_lc.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        if tok.len() >= min_tok_len && tok.starts_with(prefix_lc) {
            return true;
        }
    }
    false
}

/// 归一化字段名 / 字符串以支持 camelCase / snake_case / kebab-case 通吃 —— 全小写 + 去掉分隔符。
/// 例：`apiKey` → `apikey`；`API_KEY` → `apikey`；`api-key` → `apikey`；这样 `contains("apikey")` 命中。
fn normalize_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '_' || c == '-' {
            continue;
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// 判断 `path` 是否指向 `recent_commits` 数组元素里的自由文本字段
/// （`subject` / `author` / `files[K].path`）。
///
/// P0-4（2026-07-07）：commit subject 与 author 是用户 git 历史里的自由
/// 文本，天然可能含 `diff` / `hunk` / `prompt` 等 git 语义词——它们都在
/// `SENSITIVE_VALUE_KEYWORDS` 里，值层 `contains` 扫描会把"修了 diff 渲染"这类正常 commit
/// message 判成敏感命中，导致整份 snapshot 校验失败、同步长期挂死（用户部署即撞）。
/// v4.3+ 补：`files[K].path`（提交改动的文件路径）同理——文件名天然可能含 prompt/token 等词。
///
/// 命中此判定的字段只跳过**关键词黑名单**，仍保留 `SECRET_PREFIXES` 高熵密钥前缀兜底
/// （有人把密钥贴进 commit message / 文件名时也能拦）。
///
/// path 形如 `projects[0].recent_commits[2].subject` 或 `...recent_commits[2].files[3].path`：
/// 以 `.subject` / `.author` / `.files[<digits>].path` 结尾，且父路径以 `recent_commits[<digits>]`
/// 收尾——用锚点匹配而非全正则，避免引依赖。
fn is_recent_commit_freetext(path: &str) -> bool {
    // P0-4-b：原 `rsplit_once("recent_commits[")` 只看后缀，会把 registry 里
    // 伪造的 machine key（如 `xrecent_commits[0].subject`——machines 允许任意动态 key）误判成
    // commit 自由文本、绕过关键词扫描。改为从头精确锚定整条 path 结构
    // `projects[<digits>].recent_commits[<digits>].(subject|author)`，任何非该结构都不豁免。
    let rest = match path.strip_prefix("projects[") {
        Some(r) => r,
        None => return false,
    };
    let (pidx, rest) = match rest.split_once(']') {
        Some(p) => p,
        None => return false,
    };
    if pidx.is_empty() || !pidx.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let rest = match rest.strip_prefix(".recent_commits[") {
        Some(r) => r,
        None => return false,
    };
    let (cidx, rest) = match rest.split_once(']') {
        Some(p) => p,
        None => return false,
    };
    if cidx.is_empty() || !cidx.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if rest == ".subject" || rest == ".author" {
        return true;
    }
    // v4.3+：files[K].path 是提交改动的文件路径，同属 git 自由文本（文件名天然可能含
    // prompt/token/secret 等词），与 subject/author 同等豁免关键词黑名单、保留 SECRET_PREFIXES。
    let rest = match rest.strip_prefix(".files[") {
        Some(r) => r,
        None => return false,
    };
    let (fidx, rest) = match rest.split_once(']') {
        Some(p) => p,
        None => return false,
    };
    if fidx.is_empty() || !fidx.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    rest == ".path"
}

fn compile(s: &str) -> Validator {
    let value: serde_json::Value = serde_json::from_str(s)
        .expect("bundled schema must be valid JSON");
    jsonschema::validator_for(&value).expect("bundled schema must compile")
}

fn registry_project_validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| compile(REGISTRY_PROJECT_SCHEMA))
}

fn snapshot_validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| compile(SNAPSHOT_SCHEMA))
}

fn event_validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| compile(EVENTS_SCHEMA))
}

fn run_validator(v: &Validator, value: &serde_json::Value) -> Result<(), SchemaError> {
    let errs: Vec<String> = v.iter_errors(value).map(|e| e.to_string()).collect();
    if !errs.is_empty() {
        return Err(SchemaError::Violation(errs));
    }
    check_sensitive_fields(value)?;
    Ok(())
}

pub fn validate_registry_project(v: &serde_json::Value) -> Result<(), SchemaError> {
    run_validator(registry_project_validator(), v)
}

pub fn validate_snapshot(v: &serde_json::Value) -> Result<(), SchemaError> {
    run_validator(snapshot_validator(), v)
}

pub fn validate_event(v: &serde_json::Value) -> Result<(), SchemaError> {
    run_validator(event_validator(), v)
}

/// 递归检查字段名 / 字符串值命中敏感关键词。
/// 命中即拒（防止未来 schema 演进意外放宽 allowlist 时的隐私事故兜底）。
pub fn check_sensitive_fields(value: &serde_json::Value) -> Result<(), SchemaError> {
    let mut hits: Vec<String> = Vec::new();
    walk(value, "", &mut hits);
    if hits.is_empty() {
        Ok(())
    } else {
        Err(SchemaError::SensitiveFieldHit(hits))
    }
}

fn walk(value: &serde_json::Value, path: &str, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                // camelCase / snake_case / kebab-case 归一化后再匹配（P0-E 修复）
                let k_norm = normalize_ident(k);
                if SENSITIVE_EXACT_FIELD_NAMES.iter().any(|s| &k_norm == *s) {
                    hits.push(format!("字段名 '{child_path}' 命中敏感精确黑名单"));
                }
                for kw in SENSITIVE_KEYWORDS {
                    if k_norm.contains(kw) {
                        hits.push(format!(
                            "字段名 '{child_path}' 含敏感关键词 '{kw}'"
                        ));
                    }
                }
                walk(v, &child_path, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk(v, &format!("{path}[{i}]"), hits);
            }
        }
        // 审计对账 P0-E（2026-07-06）+ P1-C（2026-07-06）：
        //   - 字段值层用**收窄的** `SENSITIVE_VALUE_KEYWORDS`（不含 token/auth 等业务常用词）
        //   - 高熵密钥前缀用 **starts_with + 边界匹配**（原 `contains` 会误伤 `"api.xoxa-endpoint.com"`）
        serde_json::Value::String(s) => {
            let s_lc = s.to_lowercase();
            // P0-4：recent_commits[*].subject/author 是用户 commit 自由文本，
            // 跳过关键词黑名单（否则含 diff/hunk/prompt 的正常 commit message 会拒收整份 snapshot
            // 令同步挂死）；下方 SECRET_PREFIXES 兜底仍执行，防止密钥被贴进 commit message 泄露。
            if !is_recent_commit_freetext(path) {
                for kw in SENSITIVE_VALUE_KEYWORDS {
                    if s_lc.contains(kw) {
                        hits.push(format!("字符串值 '{path}' 含敏感关键词 '{kw}'"));
                    }
                }
            }
            for prefix in SECRET_PREFIXES {
                let pfx_lc = prefix.to_lowercase();
                if string_value_matches_secret_prefix(&s_lc, &pfx_lc) {
                    hits.push(format!(
                        "字符串值 '{path}' 疑似含密钥前缀 '{prefix}'"
                    ));
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug)]
pub enum SchemaError {
    Violation(Vec<String>),
    SensitiveFieldHit(Vec<String>),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Violation(errs) => {
                write!(f, "schema violation: {}", errs.join("; "))
            }
            Self::SensitiveFieldHit(hits) => {
                write!(f, "sensitive field hit: {}", hits.join("; "))
            }
        }
    }
}

impl std::error::Error for SchemaError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // P0-4 回归 + v4.3+ 扩展：files[K].path 也算 commit 自由文本，豁免关键词黑名单。
    #[test]
    fn freetext_covers_recent_commit_fields_including_files_path() {
        assert!(is_recent_commit_freetext(
            "projects[1].recent_commits[4].files[0].path"
        ));
        assert!(is_recent_commit_freetext("projects[0].recent_commits[2].subject"));
        assert!(is_recent_commit_freetext("projects[0].recent_commits[2].author"));
        // 锚点精确：只 .path 豁免，files 下其它键 / 伪造 machine key 不豁免
        assert!(!is_recent_commit_freetext(
            "projects[1].recent_commits[4].files[0].additions"
        ));
        assert!(!is_recent_commit_freetext(
            "machines[xrecent_commits[0].files[0].path"
        ));
    }

    // 本次线上撞的回归：文件路径含 prompt 等关键词应放行，但真密钥前缀仍兜底拦截。
    #[test]
    fn files_path_keyword_allowed_but_secret_prefix_still_caught() {
        let ok = json!({
            "projects": [{
                "recent_commits": [{
                    "files": [{ "path": "example-app/prompts/system.txt" }]
                }]
            }]
        });
        assert!(
            check_sensitive_fields(&ok).is_ok(),
            "含 prompt 的文件路径不应被拦：{:?}",
            check_sensitive_fields(&ok)
        );

        let leak = json!({
            "projects": [{
                "recent_commits": [{
                    "files": [{ "path": "dump/sk-ant-api03-abcdefghijklmnop.txt" }]
                }]
            }]
        });
        assert!(
            check_sensitive_fields(&leak).is_err(),
            "文件名含真密钥前缀应被兜底拦截"
        );
    }
}
