# project-dashboard

本地优先的开发看板：集中浏览 Claude Code、Codex、pi 和 Grok 的本机会话，查看跨机项目状态，并管理项目与已有 RepoWiki 内容。

**当前为 0.2.14 dogfooding 版。** 功能保持当前开发版本，不承诺生产级稳定性。首发安装包仅验证 macOS Apple Silicon；Windows、Intel Mac、Linux 和真实数据 Web 服务尚未作为发布平台验收。

## 安装与开始使用

从 [Releases](https://github.com/Snorlax-zzZ/project-dashboard-oss/releases) 下载 ARM64 DMG，核对同一 Release 的 SHA-256，然后将 App 拖到 Applications。当前使用 ad-hoc 签名，未完成 Apple 公证，系统可能提示无法验证开发者；是否允许打开请按 macOS 的系统提示自行判断，不需要关闭系统安全保护。

启动后进入「会话」，按来源切换已有本机会话。没有安装相应工具或对应目录为空时，列表为空。搜索、分页、收藏、隐藏及恢复都在本机处理。应用不修改 harness 的原始会话，不提供删除原始会话功能。

五个入口分别是会话、Fleet、项目管理、代码知识、设置。跨机同步需要 Git 和你自己的数据仓；代码知识需要额外的 RepoWiki 工具与已生成的项目内容。详见 [使用与数据说明](docs/USAGE.md)、[隐私边界](docs/PRIVACY.md) 和 [已知限制](docs/KNOWN-LIMITATIONS.md)。

**RepoWiki 工具尚待开源，目前同样处于 dogfooding 阶段。** 本仓仅包含代码知识的浏览集成，不包含 RepoWiki 工具本身。尚未具备该工具和已生成内容的用户，暂时无法使用代码知识入口；会话、Fleet、项目管理和设置仍可按各自前提使用。

## 从源码构建

需要 Rust stable（至少 1.89，文件锁 API 要求）、Git、Node.js 22 或更新 LTS、Python 3，以及 Tauri 2 CLI。macOS 构建还需 Xcode Command Line Tools。当前验证工具链为 Rust 1.95 和 Tauri CLI 2.11.4。

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

首次获取依赖需要联网。依赖完整后可设置 `DASHBOARD_OFFLINE=1` 使用本机缓存构建。图标由已审查的 SVG 生成，生成图标不纳入源码仓；安装包带完整图标。构建结果在 `target/release/bundle/dmg/`，第三方声明会随 App 打包。

CLI 可用 `cargo build -p dashboard-cli --release --locked` 单独构建，输出 `target/release/fleet`；它面向排障和采集，不包含图形界面。

## 反馈与贡献

请在 [Issues](https://github.com/Snorlax-zzZ/project-dashboard-oss/issues) 提供版本、操作系统、复现步骤及经过脱敏的截图。不要上传真实会话、访问令牌、完整本机配置或私有项目路径。涉及安全问题时，先遵循 [安全说明](SECURITY.md)。

本仓由开发仓经开源导出流水线生成。外部 PR 合并前，维护者需要先把等价改动纳入开发仓及导出规则，保证后续导出可以再生这些改动；不会直接覆盖贡献者的改动。详见 [贡献说明](CONTRIBUTING.md)。

本项目采用 [MIT License](LICENSE)。第三方组件保留各自许可证，详见 [第三方声明](THIRD-PARTY.md)。本轮发布说明见 [0.2.14 dogfooding](docs/releases/0.2.14-dogfooding.md)。
