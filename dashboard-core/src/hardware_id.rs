//! 本机硬件标识 —— machine_id 的稳定来源。
//!
//! 为什么不能继续用 hostname 当 machine_id：macOS 在局域网撞名时会自动给主机名加后缀
//! （`...Pro-3` → `...Pro-4`），machine_id 随之漂移，云端 `machines/` 下同一台机就会
//! 躺下两份互不相认的快照，共享项目注册表里按旧键存的本机路径也一并失效。
//!
//! 为什么不用 MAC 地址：实测本机所有网卡（Wi-Fi en0 与雷雳 en1–en3）的 MAC 首字节都置了
//! locally-administered 位 —— macOS「私有 Wi-Fi 地址」按网络随机化、雷雳口是桥接虚拟地址，
//! 换个网络就变，比 hostname 更不稳。
//!
//! 选定方案：取主板级设备标识（mac 的 IOPlatformUUID / win 的 MachineGuid /
//! linux 的 machine-id），只落它的 sha256 前 8 位。取哈希而非原文的原因：machine_id 会
//! 随 dashboard-state 推到远端仓库，硬件标识原文进 git 历史属于没必要的设备指纹暴露。

use sha2::{Digest, Sha256};

/// 硬件标识哈希取前 8 位（32 bit）。碰撞概率对「个人几台机器」的量级可忽略，
/// 同时保证 `machines/<id>/` 目录名足够短、可读。
const HARDWARE_HASH_HEX_LEN: usize = 8;

/// 机型关键词 → 短前缀。用于把 `demo-MacBook-Pro-4` 这类主机名压成 `mbp`。
/// 匹配在小写化后的主机名上做子串查找，长关键词必须排在短关键词之前
/// （`macbook-pro` 要先于 `macbook` 命中，否则 MacBook Pro 会被判成 `mba` 的兄弟档）。
const MODEL_KEYWORD_PREFIXES: &[(&str, &str)] = &[
    ("macbookpro", "mbp"),
    ("macbook-pro", "mbp"),
    ("macbookair", "mba"),
    ("macbook-air", "mba"),
    ("macbook", "mbp"),
    ("macmini", "mini"),
    ("mac-mini", "mini"),
    ("imac", "imac"),
    ("macstudio", "studio"),
    ("macpro", "macpro"),
    ("desktop", "desktop"),
    ("laptop", "laptop"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardwareIdError {
    /// 平台不支持（既非 mac / win / linux）。
    PlatformUnsupported,
    /// 取标识的命令跑失败 / 文件读不到。
    ProbeFailed(String),
    /// 命令跑通了但输出里没有预期字段（契约变了）。
    NotFound,
}

impl std::fmt::Display for HardwareIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlatformUnsupported => write!(formatter, "当前平台不支持取硬件标识"),
            Self::ProbeFailed(detail) => write!(formatter, "取硬件标识失败：{detail}"),
            Self::NotFound => write!(formatter, "硬件标识输出中找不到预期字段"),
        }
    }
}

impl std::error::Error for HardwareIdError {}

/// 本机硬件标识哈希（前 8 位十六进制）。
///
/// **取不到就返回 Err，绝不兜底成假身份。** 历史上前端在取不到 machine_id 时 fallback 成
/// `unknown-machine` 并照样往云端写快照，结果云端永久多出一个谁都不认的孤儿目录 ——
/// 宁可当次同步失败，也不能污染跨机数据。
pub fn hardware_hash() -> Result<String, HardwareIdError> {
    let raw = probe_hardware_identifier()?;
    Ok(hash_identifier(&raw))
}

fn hash_identifier(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.trim().to_ascii_lowercase().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..HARDWARE_HASH_HEX_LEN].to_string()
}

/// 组装 machine_id：`<短前缀>-<硬件哈希8位>`，例如 `mbp-00000002`。
///
/// 前缀只是给人看的装饰位，**认机器只认哈希那一截**；前缀在首次算出后由调用方冻结进
/// local.yaml，之后主机名再漂也不会改动它（猜错了手工改一次即可）。
pub fn compose_machine_id(prefix: &str, hardware_hash: &str) -> String {
    let prefix = sanitize_prefix(prefix);
    format!("{prefix}-{hardware_hash}")
}

/// 从主机名推短前缀：认出机型关键词就用它，认不出退回平台名。
///
/// 不用 `hw.model` 的原因：它只给 `Mac14,9` 这类代号，认不出是 MacBook Pro，
/// 要额外维护一张机型对照表，苹果每出新机型就得改表。
pub fn derive_prefix(hostname: &str) -> String {
    let normalized: String = hostname
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    for (keyword, prefix) in MODEL_KEYWORD_PREFIXES {
        if normalized.contains(keyword) {
            return (*prefix).to_string();
        }
    }
    platform_prefix().to_string()
}

/// 当次真实主机名。**这是展示字段，不是身份字段** —— 取不到只影响可读性，
/// 所以这里允许兜底成空串；硬件哈希取不到才是致命的（见 [`hardware_hash`]）。
pub fn current_hostname() -> String {
    #[cfg(target_os = "windows")]
    if let Ok(value) = std::env::var("COMPUTERNAME") {
        if !value.is_empty() {
            return value;
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 交互 shell 有 HOSTNAME env；GUI 双击启动（Finder / Dock / launchd）不继承
        // shell env，故 fallback 到 `hostname` 命令，且用绝对路径避免 PATH 过短。
        if let Ok(value) = std::env::var("HOSTNAME") {
            if !value.is_empty() {
                return value;
            }
        }
        if let Ok(output) = std::process::Command::new("/bin/hostname")
            .arg("-s")
            .output()
        {
            if let Ok(text) = String::from_utf8(output.stdout) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    String::new()
}

fn platform_prefix() -> &'static str {
    if cfg!(target_os = "windows") {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

/// 前缀落地前的清洗：只留小写字母数字，限长，空则退平台名。
/// machine_id 会当目录名用（`machines/<id>/`），不能含路径分隔符或空白。
fn sanitize_prefix(prefix: &str) -> String {
    let cleaned: String = prefix
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    if cleaned.is_empty() {
        platform_prefix().to_string()
    } else {
        cleaned
    }
}

#[cfg(target_os = "macos")]
fn probe_hardware_identifier() -> Result<String, HardwareIdError> {
    // 用绝对路径 spawn：GUI 双击启动（Finder / Dock / launchd）不继承 shell env，
    // PATH 只有系统默认那几段，相对命令名有找不到的风险。
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .map_err(|error| HardwareIdError::ProbeFailed(error.to_string()))?;
    if !output.status.success() {
        return Err(HardwareIdError::ProbeFailed(format!(
            "ioreg 退出码 {:?}",
            output.status.code()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    extract_quoted_value(&text, "IOPlatformUUID").ok_or(HardwareIdError::NotFound)
}

#[cfg(target_os = "windows")]
fn probe_hardware_identifier() -> Result<String, HardwareIdError> {
    // 待核实（2026-07-25）：本条在 mac 上无法验证，需在 Windows 机真跑一次确认
    // reg.exe 输出格式与 MachineGuid 取值。MachineGuid 由 Windows 安装时生成，
    // 重装系统会变（与 mac 的 IOPlatformUUID 语义略有差异），但不随主机名/网络漂移。
    let output = std::process::Command::new(r"C:\Windows\System32\reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .map_err(|error| HardwareIdError::ProbeFailed(error.to_string()))?;
    if !output.status.success() {
        return Err(HardwareIdError::ProbeFailed(format!(
            "reg.exe 退出码 {:?}",
            output.status.code()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    extract_reg_sz_value(&text, "MachineGuid").ok_or(HardwareIdError::NotFound)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn probe_hardware_identifier() -> Result<String, HardwareIdError> {
    // Linux 读文件即可，无需 spawn。/etc/machine-id 是 systemd 生成的稳定标识；
    // 部分发行版只有 dbus 那份，故两处都试。
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(HardwareIdError::NotFound)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn probe_hardware_identifier() -> Result<String, HardwareIdError> {
    Err(HardwareIdError::PlatformUnsupported)
}

/// 从 `ioreg` 输出里抽 `"<key>" = "<value>"` 形式的值。
/// ioreg 对字符串属性输出带引号，对二进制属性输出 `<hex>`，后者不是我们要的形状故不匹配。
fn extract_quoted_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for line in text.lines() {
        let Some(rest) = line.split_once(&needle) else {
            continue;
        };
        let after_equals = rest.1.split_once('=')?.1.trim();
        let unquoted = after_equals.strip_prefix('"')?;
        let value = unquoted.split('"').next()?.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// 从 `reg.exe query` 输出里抽 `<name>    REG_SZ    <value>` 形式的值。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extract_reg_sz_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        if !line.contains(key) {
            continue;
        }
        let value = line.rsplit("REG_SZ").next()?.trim();
        if !value.is_empty() && value != line.trim() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_hash_is_stable_and_short() {
        let first = hash_identifier("8E1B2C3D-4F56-7890-ABCD-EF1234567890");
        let second = hash_identifier("8e1b2c3d-4f56-7890-abcd-ef1234567890");
        assert_eq!(first.len(), HARDWARE_HASH_HEX_LEN);
        // 大小写与首尾空白归一化后必须同哈希：ioreg / reg.exe 输出大小写不保证一致。
        assert_eq!(first, second);
        assert_eq!(hash_identifier("  8E1B2C3D-4F56-7890-ABCD-EF1234567890 "), first);
        assert_ne!(first, hash_identifier("другой-uuid"));
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn prefix_recognizes_model_keywords_from_drifted_hostname() {
        // 真实漂移样本：后缀 -3 / -4 变了，推出的前缀必须一致。
        assert_eq!(derive_prefix("demo-MacBook-Pro-3"), "mbp");
        assert_eq!(derive_prefix("demo-MacBook-Pro-4"), "mbp");
        assert_eq!(derive_prefix("MacBookAir"), "mba");
        assert_eq!(derive_prefix("Mac-mini-of-example"), "mini");
        assert_eq!(derive_prefix("DESKTOP-DEMO01"), "desktop");
    }

    #[test]
    fn prefix_falls_back_to_platform_when_model_unknown() {
        let fallback = derive_prefix("some-random-box");
        assert_eq!(fallback, platform_prefix());
        // 中文主机名过滤后为空，也必须退到平台名而不是空串。
        assert_eq!(derive_prefix("用户的电脑"), platform_prefix());
    }

    #[test]
    fn machine_id_is_prefix_plus_hash_and_safe_as_directory_name() {
        let id = compose_machine_id("MacBook Pro!!", "00000002");
        assert_eq!(id, "macbookpro-00000002");
        assert!(!id.contains('/') && !id.contains('\\') && !id.contains(' '));
        // 空前缀不得产生以 '-' 开头的目录名。
        let empty = compose_machine_id("", "00000002");
        assert_eq!(empty, format!("{}-00000002", platform_prefix()));
    }

    #[test]
    fn ioreg_quoted_value_is_extracted_and_binary_attributes_are_ignored() {
        let sample = "  | {\n\
            \x20   \"model\" = <\"Mac14,9\">\n\
            \x20   \"IOPlatformUUID\" = \"8E1B2C3D-4F56-7890-ABCD-EF1234567890\"\n\
            \x20   \"IOPlatformSerialNumber\" = \"ABCD1234\"\n\
            }";
        assert_eq!(
            extract_quoted_value(sample, "IOPlatformUUID").as_deref(),
            Some("8E1B2C3D-4F56-7890-ABCD-EF1234567890")
        );
        // 缺字段必须返回 None，让调用方走 NotFound 而不是拿到空串当身份。
        assert!(extract_quoted_value(sample, "IOPlatformNope").is_none());
    }

    #[test]
    fn reg_sz_value_is_extracted() {
        let sample = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    \
                      MachineGuid    REG_SZ    8e1b2c3d-4f56-7890-abcd-ef1234567890\r\n";
        assert_eq!(
            extract_reg_sz_value(sample, "MachineGuid").as_deref(),
            Some("8e1b2c3d-4f56-7890-abcd-ef1234567890")
        );
        assert!(extract_reg_sz_value(sample, "OtherKey").is_none());
    }

    // 真机探测：只在能取到时断言形状，取不到（CI 容器 / 无权限）不算失败。
    #[test]
    fn real_probe_yields_stable_hash_when_available() {
        match hardware_hash() {
            Ok(hash) => {
                assert_eq!(hash.len(), HARDWARE_HASH_HEX_LEN);
                assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
                // 连取两次必须一致 —— 这是「稳定」的最小证明。
                assert_eq!(Some(hash), hardware_hash().ok());
            }
            Err(error) => {
                eprintln!("本环境取不到硬件标识（不算失败）：{error}");
            }
        }
    }
}
