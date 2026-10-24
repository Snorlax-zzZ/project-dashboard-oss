# project-dashboard

本地优先的开发看板：集中浏览九种工具的本机会话，查看跨机项目状态，并管理项目与已有 RepoWiki 内容。支持 Claude Code、Codex、pi、Grok、OpenCode、Antigravity CLI、DSH、ZCode 和 Gemini CLI。

**当前为 0.2.20 dogfooding 版。** 支持 macOS Apple Silicon 和 Windows x64 构建，不承诺生产级稳定性。开发版本已在这两个平台进行功能验证；可用公开安装包及其验证范围以对应 Release 为准。Intel Mac、Linux、干净 Windows 和真实双机仍有待验范围；本仓不是实际会话的 Web 服务。

## 安装与开始使用

从 [Releases](https://github.com/Snorlax-zzZ/project-dashboard-oss/releases) 选择已提供的对应平台安装包，核对同一 Release 的 SHA-256。macOS 使用 ARM64 DMG，将 App 拖到 Applications；当前为 ad-hoc 签名、未完成 Apple 公证，请按系统提示自行判断是否打开，不需要关闭系统安全保护。Windows 使用 x64 NSIS；没有附件时可从源码构建，开发环境的验收不代表该公开安装包已验收。

进入「会话看板」，按来源切换本机历史。对应目录为空时显示空列表；格式异常或数据库读取失败会提示错误。搜索、分页、收藏、隐藏及恢复都在本机处理。来源入口支持显隐、排序及拖拽；设置中可切换简体中文和 English。应用不修改原工具的会话，不提供删除原始会话功能。

五个入口是会话看板、协作看板、项目管理、代码知识和设置。跨机同步需要 Git 与自己的数据仓；代码知识需要额外的 RepoWiki 工具与已生成内容。详见 [使用与数据说明](docs/USAGE.md)、[会话来源](docs/session-sources.md)、[界面语言](docs/interface-language.md)、[隐私边界](docs/PRIVACY.md) 和 [已知限制](docs/KNOWN-LIMITATIONS.md)。

**RepoWiki 工具尚待开源，目前同样处于 dogfooding 阶段。** 本仓只包含浏览集成，不包含 RepoWiki 本体。没有兼容工具及已生成内容时，暂时不能使用代码知识；其他入口仍可按各自前提使用。

## 从源码构建

需要 Rust stable（至少 1.89，文件锁 API 要求）、Git、Node.js 22 或更新 LTS、Python 3 和 Tauri 2 CLI。macOS 还需 Xcode Command Line Tools；Windows 还需 MSVC C++ 构建工具、Windows SDK 与 WebView2。开发验证工具链为 Rust 1.95、Tauri CLI 2.11.4。

```sh
git clone https://github.com/Snorlax-zzZ/project-dashboard-oss.git
cd project-dashboard-oss
cargo install tauri-cli --version 2.11.4 --locked
cargo fetch --locked --target aarch64-apple-darwin
cargo tauri icon dashboard-tauri/icons/app-icon.svg --output dashboard-tauri/icons
node --test dashboard-tauri/tests/*.test.js
cargo test --workspace --locked
bash scripts/build-macos.sh
```

首次获取依赖需要联网。依赖齐全后可设置 `DASHBOARD_OFFLINE=1` 使用缓存。图标从纯 SVG 生成，生成图标不纳入源码仓；安装包带完整图标。macOS 产物在 `target/release/bundle/dmg/`，第三方声明随 App 打包。

Windows PowerShell 构建（先完成 clone 和 CLI 安装）：

```powershell
cargo fetch --locked --target x86_64-pc-windows-msvc
cargo tauri icon dashboard-tauri/icons/app-icon.svg --output dashboard-tauri/icons
$tests = (Get-ChildItem dashboard-tauri/tests -Filter '*.test.js').FullName
node --test @tests
cargo test --workspace --locked
python scripts/build_windows.py
```

产物在 `target/release/bundle/nsis/`。构建脚本生成图标、收集当前平台的许可证并重映射编译路径。SQLite 与 zstd 随应用内嵌，用户无需安装额外运行时。公开应用标识在两个平台均保持 `io.github.snorlaxzzz.projectdashboard`，与 0.2.14 公开版一致。

CLI 可用 `cargo build -p dashboard-cli --release --locked` 单独构建，输出 `target/release/fleet`（Windows 为 `fleet.exe`）；用于排障和采集，不包含图形界面。

## 反馈与贡献

请在 [Issues](https://github.com/Snorlax-zzZ/project-dashboard-oss/issues) 提供版本、操作系统、复现步骤及脱敏截图。不要上传真实会话、令牌、完整本机配置或私有项目路径。安全问题请先阅读 [安全说明](SECURITY.md)。

本仓由开发仓经开源导出流水线生成。外部 PR 合并前，维护者需要把等价改动纳入开发仓及导出规则，保证后续可以再生，避免覆盖贡献者的改动。详见 [贡献说明](CONTRIBUTING.md)。

本项目采用 [MIT License](LICENSE)，第三方组件保留各自许可证，见 [第三方声明](THIRD-PARTY.md)。本轮说明见 [0.2.20 dogfooding](docs/releases/0.2.20-dogfooding.md)，历史说明见 [0.2.14 dogfooding](docs/releases/0.2.14-dogfooding.md)。
