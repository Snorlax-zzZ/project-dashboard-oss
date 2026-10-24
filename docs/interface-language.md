# 界面语言

在「设置 → 界面语言」选择「简体中文」或「English」，立即生效。首次启动默认中文。

选择仅记录在当前 App WebView 的本机 localStorage（键为 `project-dashboard.language`），不进入 `dashboard-state`，也不会修改各工具的会话。重启后恢复；本机存储不可用时仍可在当前窗口切换，并提示无法保存。

五个入口、状态、分页、来源管理、项目表单、操作确认、Wiki 查找和图谱操作支持中英文。会话标题与对话、项目名、路径、Git 提交、Wiki 正文和原始诊断保持原文。跨机同步开关、收藏、隐藏和来源排序沿用原有数据与接口。

切换语言不重载窗口、不发起业务 IPC，保留搜索词、分页、已输入表单和会话详情。关闭或切换业务页时仍按原来的规则关闭详情。

## 开发约定

- `dashboard-tauri/dist/i18n.js` 提供离线英文词典及本机偏好，先于外观和业务脚本加载。
- 静态节点使用 `data-i18n`；属性使用 `data-i18n-title`、`data-i18n-placeholder`、`data-i18n-aria-label`。只标记产品文案，不标记用户内容。
- JavaScript 产品字面量使用 `uiText('源文案')`，动态消息与 HTML 使用 `uiTemplate` 标签模板。源文案是字典键，模板参数按 `{0}`、`{1}` 编号。新增或调整 HTML 模板时须同步完整词典键。
- 参数不经过二次翻译或替换；写入 HTML 的数据仍须在业务渲染处使用 `escapeHtml`。
- 已产生的会话和同步提示可保留纯展示渲染函数，以便换语言时更新结果；渲染函数不得访问 IPC 或重新执行操作。
- 语言切换重绘只使用缓存。避免调用 `switchTab`、`refreshAll`、`loadSettings` 或 RepoWiki 读页函数。
- Node 用例覆盖词典参数、静态文案和原文隔离。运行 `node dashboard-tauri/tests/layout-server.cjs` 后，可在打印的本机地址访问 `/i18n.html` 做合成数据回归；测试进程会自动限时退出。
