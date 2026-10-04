// project-dashboard 前端逻辑（vanilla + Tauri IPC）
//
// 说明：Tauri v2 通过全局 `window.__TAURI__.core.invoke(cmd, args)` 暴露 IPC。
// 本文件在**非 Tauri 环境**（如浏览器直开 index.html）时降级到 mock，
// 方便审计时无 Tauri runtime 也能看到交互流程。

const invoke = (cmd, args) => {
  if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
    return window.__TAURI__.core.invoke(cmd, args);
  }
  return mockInvoke(cmd, args);
};

// RepoWiki v2 动态目录：不再有固定 10 页白名单。每个项目的页面集合由后端 navigation
// （扁平数组 + parent_page_id/order 表达树形）在读取时动态构建，存入 state.repowiki
// （pages/pageIndex/pageIds/homePageId）。页面跳转白名单以 state.repowiki.pageIds 为准。
const REPOWIKI_NODE_ID = /^[0-9a-f]{64}$/;
// 「人工处理命令」面板已移除（2026-07-27）：Dashboard 的定位是只读查看，不指导操作。
// 那套面板三重失效——① 命令写死 v1 的 prepare/publish，而 v2 wiki 要走 `repowiki wiki *`，
// 给的是错命令；② 路径全是占位符，复制出来也跑不了；③ 过期时只给 `status`，那只是再查一遍。
// 该跑什么由 RepoWiki CLI 自己负责，Dashboard 猜命令只会误导。

// -------- State --------
const SESSION_SOURCE_IDS = Object.freeze(['claude-cli', 'codex', 'pi', 'grok']);

// 页签显示名。来源 id 是后端 allowlist 里的稳定标识，展示名跟它解耦（如 'claude-cli' → 'Claude Code'）。
const SESSION_SOURCE_LABELS = Object.freeze({
  'claude-cli': 'Claude Code',
  codex: 'Codex',
  pi: 'pi',
  grok: 'Grok',
});

function createSessionView() {
  return { items: [], query: '', page: 1, pageSize: 20, total: 0, error: '', loading: false, visibility: 'visible', notice: '', needsRefresh: false, pendingHidden: new Set(), pendingFavorites: new Set() };
}

function createSessionViews() {
  // 每个来源一个独立状态对象：切页签不串搜索词 / 页码 / 结果
  return Object.fromEntries(SESSION_SOURCE_IDS.map(id => [id, createSessionView()]));
}

let state = {
  activeTab: 'sessions',
  // Session Board 页面级来源页签。每个来源独立保留搜索 / 分页 / 结果，切换不串状态。
  sessionSource: 'claude-cli',
  sessionViews: createSessionViews(),
  sessionListRequestSerial: Object.fromEntries(SESSION_SOURCE_IDS.map(id => [id, 0])),
  sessionDetailRequestSerial: 0,
  sessionDetailKey: '',
  sessionHideConfirmationOpen: false,
  // v4.2 §16.6：`pending` 通道已删除；UUID 权威在 dashboard-state registry。
  projects: { shared: [], private: [] },
  // projectsLoaded：区分"registry 为空" vs "registry 还没拉取完"，避免空态误报（对账 #3）
  projectsLoaded: false,
  aggregate: [],
  // aggregateError：refreshAggregateSilently 失败时置为 error 消息，UI 顶部横幅提示，
  // 不再仅 console.warn 让用户看陈旧数据无感知（对账 #6）
  aggregateError: null,
  editingProjectId: null,
  // notice 表示最近同步的非成功结局；启动/刷新从后端本机记录恢复，state 只作当前视图。
  // updateSyncStatus 渲染时 notice 优先于 last——否则后续任意 refreshAll 会用旧 last 把失败态复活成假成功。
  syncStatus: { last: null, error: null, notice: null },
  syncStatusRequestSerial: 0,
  syncInProgress: false,
  syncRecordWarning: null,
  // machine_id 由 backend `get_local_context` 提供（审计对账 P0-D，2026-07-06）
  //  —— 前端不再硬编码 'DESKTOP-DEMO01'，跨机器时前端渲染跟随本机 machine_id。
  machineId: '',
  /// 身份取不到时的原因（后端给的面向用户文案）。非空即代表本机处于「身份不可用」硬故障态。
  machineIdError: '',
  /// 当次真实主机名，展示用（与 machineId 分离，后者不随主机名漂移）。
  hostname: '',
  os: '',
  repowiki: {
    selectorMap: new Map(),
    selectedKey: '',
    requestSerial: 0,
    graphRequestSerial: 0,
    status: null,
    project: null,
    generations: null,
    currentPageId: '',
    // v2 动态目录：当前项目的页面集合（读取导航后由 validatedRepoWikiNavigation 填充）。
    pages: [],                     // 有序扁平数组：{page_id,title,path,page_type,parent_page_id,order}
    pageIndex: new Map(),          // Map<page_id, page>：按 id 直取
    pageIds: new Set(),            // Set<page_id>：页面跳转与读页的唯一白名单
    pathIndex: new Map(),          // Map<规范化 path, page_id>：正文里的相对 .md 链接靠它回查页面
    homePageId: '',                // 根页面 id（parent_page_id 为 null 的那一页）
    expandedPageIds: new Set(),    // 树形导航当前展开的父节点集合（默认只展开第一层）
  },
};

// 陈旧阈值 —— 与 dashboard-core::aggregate::DEFAULT_STALE_THRESHOLD 保持一致（24h）。
// 对账 backlog #15：spec 121 写"陈旧 > 6h"是**顶部红条告警上限**，实现的 24h 是**降色阈值**，两者不同事件，
// 目前 stale 阈值统一走 24h（红条 6h 上限在 sync_status 顶栏已实现）。M4 阈值可配接入 settings 时统一。
const STALE_THRESHOLD_MS = 24 * 3600 * 1000;

// -------- Bootstrap --------
if (typeof document !== 'undefined') {
  document.addEventListener('DOMContentLoaded', async () => {
    bindNav();
    bindSyncButtons();
    bindProjectForm();
    bindTableScrollControls();
    bindDrawerClose();
    bindDrawerResize();
    bindRepoWiki(); // 只绑定 UI 事件；这里绝不调用 RepoWiki IPC。
    await bootstrapContext();
    await refreshAll();
  });
}

// drawer 关闭三重兜底：X 按钮 addEventListener（inline onclick 在 tauri v2 CSP 下不稳）
// + Esc 键 + 点标题栏文档区外部空白（不含 drawer 内 / 表格行内 / nav 内，避免误关）
function bindDrawerClose() {
  const drawer = document.getElementById('drawer');
  const closeBtn = document.getElementById('drawer-close');
  if (closeBtn) closeBtn.addEventListener('click', closeDrawer);
  document.addEventListener('keydown', (e) => {
    if (document.getElementById('confirm-backdrop')?.classList.contains('open') ||
        document.getElementById('project-modal-backdrop')?.classList.contains('open')) return;
    if (e.key === 'Escape' && drawer && drawer.classList.contains('open')) {
      closeDrawer();
    }
  });
}

// v4.3.1：drawer 左缘拖拽调宽 —— drawer 宽度与主区右缩量共用 CSS 变量 --drawer-width，
// 拖拽时实时改该变量即可联动。宽度 clamp 到 [360, 视口 72%]，避免拖没或盖满主区。
function bindDrawerResize() {
  const drawer = document.getElementById('drawer');
  const handle = document.getElementById('drawer-resizer');
  if (!drawer || !handle) return;
  const MIN = 360;
  const maxWidth = () => Math.max(MIN, Math.round(window.innerWidth * 0.72));
  let startX = 0, startWidth = 0, dragging = false;
  const onMove = (e) => {
    if (!dragging) return;
    // drawer 贴右侧：指针左移（clientX 变小）= 变宽
    const delta = startX - e.clientX;
    const w = Math.min(maxWidth(), Math.max(MIN, startWidth + delta));
    document.documentElement.style.setProperty('--drawer-width', w + 'px');
  };
  const onUp = () => {
    if (!dragging) return;
    dragging = false;
    document.body.classList.remove('drawer-resizing');
    document.removeEventListener('mousemove', onMove);
    document.removeEventListener('mouseup', onUp);
  };
  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    dragging = true;
    startX = e.clientX;
    startWidth = drawer.offsetWidth;
    document.body.classList.add('drawer-resizing');
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  });
}

async function bootstrapContext() {
  try {
    const ctx = await invoke('get_local_context', {});
    // 身份取不到时后端回 machine_id=null + machine_id_error。**绝不在此兜底编一个假 id** ——
    // 旧实现兜底成 'unknown-machine' 还照样往云端写快照，结果 machines/ 下永久多出一个
    // 谁都不认的孤儿目录（真机实测已存在一个）。这里改为置空 + 挂起阻塞级错误横幅。
    state.machineId = ctx.machine_id || '';
    state.machineIdError = ctx.machine_id_error || '';
    state.hostname = ctx.hostname || '';
    state.os = ctx.os || '';
    // 侧栏底部版本号（后端 get_local_context 用 CARGO_PKG_VERSION 注入 workspace 统一版本）
    const verEl = document.getElementById('app-version');
    if (verEl && ctx.version) verEl.textContent = 'v' + ctx.version;
    if (!state.machineId) renderMachineIdentityFailure(state.machineIdError);
  } catch (e) {
    console.error('get_local_context', e);
    state.machineId = '';
    state.machineIdError = String(e && e.message ? e.message : e);
    renderMachineIdentityFailure(state.machineIdError);
  }
}

// 身份不可用是硬故障：同步与项目登记都按 machine_id 匹配路径，带着假身份跑只会污染跨机数据。
// 故这里给一条不可关闭的顶部红条，并禁用会写云端的入口。
function renderMachineIdentityFailure(detail) {
  const banner = document.getElementById('machine-identity-banner');
  if (banner) {
    banner.style.display = '';
    banner.textContent = `无法确定本机身份：${detail || '未知原因'} —— 同步与项目登记已停用，避免写入错误身份。`;
  }
  // 只禁「会写云端」的两个入口：手动同步与新增项目。只读浏览照常可用。
  for (const id of ['btn-sync', 'btn-add-project']) {
    const el = document.getElementById(id);
    if (el) el.disabled = true;
  }
}

// -------- Nav & Tab routing --------
function bindTableScrollControls() {
  for (const [id, label] of [['session-table', '会话列表'], ['fleet-table', 'Fleet 列表'], ['project-table', '项目列表']]) {
    const table = document.getElementById(id);
    const viewport = table.parentElement;
    viewport.id = `${id}-viewport`;
    viewport.setAttribute('role', 'region');
    viewport.setAttribute('aria-label', `${label}，可用左右方向键滚动列`);
    const controls = document.createElement('div');
    controls.className = 'table-scroll-controls';
    controls.hidden = true;
    controls.innerHTML = `<span>左右滚动查看更多列</span>
      <button type="button" class="btn ghost" data-scroll="left" aria-label="向左滚动${label}" aria-controls="${viewport.id}">←</button>
      <button type="button" class="btn ghost" data-scroll="right" aria-label="向右滚动${label}" aria-controls="${viewport.id}">→</button>`;
    viewport.before(controls);
    const left = controls.querySelector('[data-scroll="left"]');
    const right = controls.querySelector('[data-scroll="right"]');
    const update = () => {
      const max = viewport.scrollWidth - viewport.clientWidth;
      controls.hidden = max <= 1;
      viewport.tabIndex = max > 1 ? 0 : -1;
      left.setAttribute('aria-disabled', String(viewport.scrollLeft <= 1));
      right.setAttribute('aria-disabled', String(viewport.scrollLeft >= max - 1));
    };
    const move = direction => {
      viewport.scrollLeft += direction * Math.max(120, viewport.clientWidth * 0.7);
      update();
    };
    left.addEventListener('click', () => move(-1));
    right.addEventListener('click', () => move(1));
    viewport.addEventListener('scroll', update, { passive: true });
    viewport.addEventListener('keydown', event => {
      if (event.target !== viewport) return; // 不接管行内按钮的键盘行为。
      if (event.key === 'ArrowLeft') move(-1);
      else if (event.key === 'ArrowRight') move(1);
      else if (event.key === 'Home') viewport.scrollLeft = 0;
      else if (event.key === 'End') viewport.scrollLeft = viewport.scrollWidth;
      else return;
      event.preventDefault();
      update();
    });
    const observer = new ResizeObserver(update);
    observer.observe(viewport);
    observer.observe(table);
    update();
  }
}

function bindNav() {
  document.querySelectorAll('.nav-item').forEach(el => {
    el.addEventListener('click', () => switchTab(el.dataset.tab));
  });
  const sourceTabs = [...document.querySelectorAll('.session-source-tab')];
  sourceTabs.forEach((el, index) => {
    el.addEventListener('click', () => switchSessionSource(el.dataset.sessionSource));
    el.addEventListener('keydown', (event) => {
      let target = index;
      if (event.key === 'ArrowRight') target = (index + 1) % sourceTabs.length;
      else if (event.key === 'ArrowLeft') target = (index - 1 + sourceTabs.length) % sourceTabs.length;
      else if (event.key === 'Home') target = 0;
      else if (event.key === 'End') target = sourceTabs.length - 1;
      else return;
      event.preventDefault();
      sourceTabs[target].focus();
      switchSessionSource(sourceTabs[target].dataset.sessionSource);
    });
  });
  // 搜索：debounce 300ms 后重新拉第 1 页
  document.getElementById('session-query').addEventListener('input', debounce(() => {
    const view = activeSessionView();
    view.query = document.getElementById('session-query').value || '';
    fetchSessions(1, view.pageSize);
  }, 300));
  document.getElementById('session-visibility').addEventListener('change', e => switchSessionVisibility(e.target.value));
  // 页大小切换：回到第 1 页
  document.getElementById('session-page-size').addEventListener('change', (e) => {
    const size = parseInt(e.target.value, 10) || 20;
    fetchSessions(1, size);
  });
  document.getElementById('project-query').addEventListener('input', debounce(renderProjects, 200));
  document.getElementById('btn-add-project').addEventListener('click', () => openProjectModal(null));
  // v4.2 补丁（Mac WKWebView 严格 CSP L3）：'unsafe-inline' 不覆盖 inline event handler，
  // 项目管理表格的编辑/删除按钮改用事件委托，避免 renderProjects 每次重绑造成的漏绑。
  document.querySelector('#project-table tbody').addEventListener('click', (e) => {
    const btn = e.target.closest('button[data-action]');
    if (!btn || btn.disabled) return;
    const id = btn.dataset.projectId || '';
    if (btn.dataset.action === 'edit') {
      editProject(id);
    } else if (btn.dataset.action === 'delete') {
      const name = btn.dataset.projectName || '';
      deleteProject(id, name);
    }
  });
}

function activeSessionView() {
  return state.sessionViews[state.sessionSource];
}

function sessionSourceLabel(source) {
  return SESSION_SOURCE_LABELS[source] || source;
}

function switchSessionSource(source) {
  if (!SESSION_SOURCE_IDS.includes(source) || source === state.sessionSource) return;
  const queryInput = document.getElementById('session-query');
  activeSessionView().query = queryInput.value || '';
  state.sessionSource = source;
  const targetView = activeSessionView();

  document.querySelectorAll('.session-source-tab').forEach((el) => {
    const active = el.dataset.sessionSource === source;
    el.classList.toggle('active', active);
    el.setAttribute('aria-selected', active ? 'true' : 'false');
    el.tabIndex = active ? 0 : -1;
  });
  const panel = document.getElementById('session-source-panel');
  if (panel) {
    const activeTab = document.querySelector(`.session-source-tab[data-session-source="${source}"]`);
    if (activeTab) panel.setAttribute('aria-labelledby', activeTab.id);
  }
  queryInput.value = targetView.query;
  document.getElementById('session-visibility').value = targetView.visibility;
  if (targetView.needsRefresh) { targetView.items = []; targetView.loading = true; }
  document.getElementById('session-page-size').value = String(targetView.pageSize);
  closeDrawer();
  renderSessions();
  renderPagination();
  fetchSessions(targetView.page, targetView.pageSize);
}

function switchSessionVisibility(visibility) {
  if (!['visible', 'hidden'].includes(visibility)) return;
  const view = activeSessionView();
  if (view.visibility === visibility) return;
  state.sessionListRequestSerial[state.sessionSource] += 1;
  view.visibility = visibility;
  view.page = 1;
  view.items = [];
  view.total = 0;
  view.notice = '';
  document.getElementById('session-visibility').value = visibility;
  closeDrawer();
  renderPagination();
  return fetchSessions(1, view.pageSize);
}

// 隐藏前确认，避免与收藏相邻时误触；恢复显示仍直接执行。
async function requestSessionHidden(source, session, hidden) {
  if (!hidden) return setSessionHidden(source, session.session_id, false);
  const view = state.sessionViews[source];
  if (!view || state.sessionHideConfirmationOpen || view.pendingHidden.has(session.session_id)) return;
  const sessionId = session.session_id;
  const title = String(session.title || '无标题').replace(/\s+/g, ' ').slice(0, 160);
  state.sessionHideConfirmationOpen = true;
  try {
    const confirmed = await uiConfirm(
      `${sessionSourceLabel(source)} · ${title}\n会话 ID：${sessionId}\n\n仅从本看板的正常列表隐藏，不会删除或归档源会话，也不影响收藏。\n之后可切换到“已隐藏”，点击“恢复显示”。`,
      { title: '隐藏此会话？', okText: '确认隐藏', cancelFirst: true },
    );
    if (!confirmed || state.activeTab !== 'sessions' || state.sessionSource !== source || view.visibility !== 'visible') return;
    await setSessionHidden(source, sessionId, true);
    if (state.activeTab === 'sessions' && state.sessionSource === source && document.activeElement === document.body) {
      document.getElementById('session-visibility').focus();
    }
  } finally {
    state.sessionHideConfirmationOpen = false;
  }
}

// 写操作捕获原来源和 ID；切换页面时不刷新其他来源或关闭其他会话的抽屉。
async function setSessionHidden(source, sessionId, hidden) {
  const view = state.sessionViews[source];
  if (!view || view.pendingHidden.has(sessionId)) return;
  view.pendingHidden.add(sessionId);
  view.notice = '';
  view.loading = false;
  state.sessionListRequestSerial[source] += 1;
  if (state.sessionSource === source) renderSessions();
  let saved = false;
  try {
    await invoke('set_session_hidden', { source, sessionId, hidden });
    saved = true;
    state.sessionListRequestSerial[source] += 1;
    view.loading = false;
    view.needsRefresh = true;
    if (state.sessionDetailKey === `${source}:${sessionId}`) closeDrawer();
    if (state.sessionSource === source) {
      const refreshed = await fetchSessions(view.page, view.pageSize);
      if (refreshed === false) view.notice = `${hidden ? '隐藏' : '恢复显示'}已保存，列表刷新失败，请重新读取`;
    }
  } catch (error) {
    view.notice = saved ? '偏好已保存，列表刷新失败，请重新读取' : `显示偏好保存失败：${error}`;
  } finally {
    view.pendingHidden.delete(sessionId);
    if (state.sessionSource === source) renderSessions();
  }
}

async function toggleSessionFavorite(source, sessionId) {
  const view = state.sessionViews[source];
  if (!view || view.pendingFavorites.has(sessionId)) return;
  view.pendingFavorites.add(sessionId);
  view.notice = '';
  view.loading = false;
  state.sessionListRequestSerial[source] += 1;
  if (state.sessionSource === source) renderSessions();
  let saved = false;
  try {
    await invoke('toggle_favorite', { sessionId });
    saved = true;
    state.sessionListRequestSerial[source] += 1;
    view.loading = false;
    view.needsRefresh = true;
    if (state.sessionSource === source && await fetchSessions(view.page, view.pageSize) === false) {
      view.notice = '收藏已保存，列表刷新失败，请重新读取';
    }
  } catch (error) {
    view.notice = saved ? '收藏已保存，列表刷新失败，请重新读取' : `收藏保存失败：${error}`;
  } finally {
    view.pendingFavorites.delete(sessionId);
    if (state.sessionSource === source) renderSessions();
  }
}

function switchTab(tab) {
  state.activeTab = tab;
  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.tab === tab);
  });
  document.querySelectorAll('.page').forEach(el => {
    el.classList.toggle('hidden', el.id !== 'page-' + tab);
  });
  // 切 tab 时兜底关掉右侧详情抽屉 —— 抽屉是当前 tab 的辅助面板（Session Board 抽屉 /
  // Fleet Board 项目详情抽屉），切走后不该继续飘在新 tab 上下文里。
  closeDrawer();
  if (tab === 'settings') loadSettings();
  // 唯一的页面级自动触发点：用户显式切入代码知识页后读取一次。
  if (tab === 'repowiki') enterRepoWikiTab();
  // Fleet Board 切入时静默拉一次 aggregate，避免后台采集器落库后 UI 陈旧。
  // 静默失败：出错只 console.warn 不打扰用户（用户点"重新读取"按钮才是显式反馈）。
  if (tab === 'fleet') refreshAggregateSilently();
}

// -------- 代码知识 / RepoWiki（严格按用户动作触发） --------

function repoWikiSelectorEntries(projects, machineId = '') {
  const entries = [];
  const shared = Array.isArray(projects?.shared) ? projects.shared : [];
  const privateProjects = Array.isArray(projects?.private) ? projects.private : [];
  for (const project of shared) {
    const projectId = project?.project_id || project?.projectId;
    const machines = project?.machines && typeof project.machines === 'object'
      ? project.machines
      : {};
    if (typeof projectId !== 'string' || !projectId.trim()
      || typeof machineId !== 'string' || !machineId
      || typeof machines[machineId] !== 'string' || !machines[machineId].trim()) continue;
    entries.push({
      key: `rw-project-${entries.length}`,
      label: `[共享] ${String(project?.name || projectId)}`,
      selector: { kind: 'shared', projectId },
    });
  }
  for (const project of privateProjects) {
    const name = project?.name;
    if (typeof name !== 'string' || !name.trim()) continue;
    entries.push({
      key: `rw-project-${entries.length}`,
      label: `[本机] ${name}`,
      selector: { kind: 'private', name },
    });
  }
  return entries;
}

function bindRepoWiki() {
  const projectSelect = document.getElementById('repowiki-project-select');
  const refreshButton = document.getElementById('btn-repowiki-refresh');
  const pageNav = document.getElementById('repowiki-page-nav');
  const article = document.getElementById('repowiki-article');
  const graphForm = document.getElementById('repowiki-graph-search-form');
  const graphResults = document.getElementById('repowiki-graph-results');

  projectSelect.addEventListener('change', () => {
    state.repowiki.selectedKey = projectSelect.value;
    loadRepoWikiProject();
  });
  refreshButton.addEventListener('click', loadRepoWikiProject);
  pageNav.addEventListener('click', (event) => {
    // 树形导航两类可点元素：折叠开关（展开/收起子节点）与页面按钮（读取正文）。
    const toggle = event.target.closest('button[data-toggle-page-id]');
    if (toggle) { toggleRepoWikiNavNode(toggle.dataset.togglePageId); return; }
    const button = event.target.closest('button[data-page-id]');
    if (button) loadRepoWikiPage(button.dataset.pageId);
  });
  article.addEventListener('click', (event) => {
    const pageLink = event.target.closest('button.wiki-page-link[data-page-id]');
    if (pageLink) loadRepoWikiPage(pageLink.dataset.pageId);
  });
  graphForm.addEventListener('submit', (event) => {
    event.preventDefault();
    searchRepoWikiGraph();
  });
  graphResults.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-graph-action][data-node-id]');
    if (button) runRepoWikiGraphOperation(button.dataset.graphAction, button.dataset.nodeId);
  });
  document.getElementById('repowiki-notice').addEventListener('click', (event) => {
    if (event.target.closest('[data-repowiki-open-settings]')) switchTab('settings');
  });
}

async function enterRepoWikiTab() {
  renderRepoWikiProjectOptions();
  if (!selectedRepoWikiSelector()) {
    renderRepoWikiNoProjects();
    return;
  }
  await loadRepoWikiProject();
}

function renderRepoWikiProjectOptions() {
  const select = document.getElementById('repowiki-project-select');
  const previous = state.repowiki.selectedKey;
  const entries = repoWikiSelectorEntries(state.projects, state.machineId);
  state.repowiki.selectorMap = new Map(entries.map(entry => [entry.key, entry.selector]));
  select.replaceChildren();
  for (const entry of entries) {
    const option = document.createElement('option');
    option.value = entry.key; // option 中只有不透明 key；UUID/name/path 均不从 DOM 反解析。
    option.textContent = entry.label;
    select.appendChild(option);
  }
  const selectedKey = state.repowiki.selectorMap.has(previous)
    ? previous
    : (entries[0]?.key || '');
  state.repowiki.selectedKey = selectedKey;
  select.value = selectedKey;
  select.disabled = entries.length === 0;
  document.getElementById('btn-repowiki-refresh').disabled = entries.length === 0;
}

function selectedRepoWikiSelector() {
  const selector = state.repowiki.selectorMap.get(state.repowiki.selectedKey);
  return selector ? { ...selector } : null;
}

function emptyStateHtml(title, hint) {
  return `<div class="empty-state compact">
    <div class="empty-title">${escapeHtml(title)}</div>
    <div class="empty-hint">${escapeHtml(hint)}</div>
  </div>`;
}

function resetRepoWikiSummary() {
  document.getElementById('repowiki-branch').textContent = '—';
  document.getElementById('repowiki-generation').textContent = '—';
  document.getElementById('repowiki-wiki-state').textContent = '状态：—';
  document.getElementById('repowiki-generation-time').textContent = '生成时间：当前契约未提供';
  document.getElementById('repowiki-graph-state').textContent = '—';
  document.getElementById('repowiki-graph-generation').textContent = '代次：—';
  document.getElementById('repowiki-review-total').textContent = '—';
  document.getElementById('repowiki-review-detail').textContent = '—';
}

// 版本比对只要能一眼看出「不一样」，不需要完整 40 位 sha / 整串代次。
// commit 取前 7 位；`rwgen_xxx_yyy` / `rwwiki_xxx_yyy` 这类代次取前缀后的第一段。
function briefVersion(value) {
  const text = String(value || '').trim();
  if (!text) return '—';
  const generation = text.match(/^rw[a-z]+_([^_\s]+)/i);
  if (generation) return generation[1];
  return /^[0-9a-f]{20,}$/i.test(text) ? text.slice(0, 7) : text;
}

function repoWikiGeneratedLabel(value) {
  const time = value ? new Date(value) : null;
  return time && !Number.isNaN(time.getTime())
    ? time.toLocaleString('zh-CN', { hour12: false })
    : '';
}

// v2 wiki 的真实状态只能从直读代次（generations.wiki）算，**不能信 status.wiki** —— CLI 的
// status 走 v1 校验器，对 v2 的 docs/ 树恒报 damaged、代次 null（它在找 v1 那 10 个固定页文件）。
// 状态卡与恢复命令必须共用这一份判定，否则会出现「卡片显示已过期、横幅却说需要人工处理」这种
// 自相矛盾的界面 —— 那正是两边各读各的所致。
function resolveRepoWikiWikiState(status, generations) {
  const generationWiki = generations?.wiki?.generation_id ? generations.wiki : null;
  return generationWiki
    ? { ...generationWiki, freshness: repoWikiV2Freshness(generationWiki, status) }
    : (status?.wiki || {});
}

// 一句话说清「这份 Wiki 现在能不能信」，附版本比对与生成日期，仅此而已。
// 不给操作指引：该跑什么命令由 RepoWiki CLI 负责，Dashboard 只是查看器。
// 返回 null = 一切正常，调用方走默认的 info 提示。
function repoWikiStatusNotice(status, generations) {
  if (!status || typeof status !== 'object') return null;
  if (status.initialization !== 'initialized') {
    return { kind: 'info', message: '该项目还没有初始化 RepoWiki。' };
  }
  const wiki = resolveRepoWikiWikiState(status, generations);
  if (wiki.freshness === 'absent') {
    return { kind: 'info', message: '该项目还没有生成正式 Wiki。' };
  }
  const generatedAt = repoWikiGeneratedLabel(wiki.generated_at);
  const generatedPart = generatedAt ? `这份 Wiki 生成于 ${generatedAt}` : '这份 Wiki';
  if (wiki.freshness === 'damaged' || wiki.integrity === 'damaged') {
    return { kind: 'warn', message: `${generatedPart}，完整性校验没通过，内容可能不可信。` };
  }
  if (wiki.freshness !== 'stale') return null;

  // 版本比对：只列真正变了的那项，让人一眼看出是「代码改了」还是「图谱重建了」。
  const changes = [];
  const currentHead = status.repository?.head_commit || status.graph?.current_source_commit;
  if (wiki.head_commit && currentHead && wiki.head_commit !== currentHead) {
    changes.push(`源码从 ${briefVersion(wiki.head_commit)} 变成了 ${briefVersion(currentHead)}`);
  }
  const currentGraph = status.graph?.generation_id;
  if (wiki.graph_generation_id && currentGraph && wiki.graph_generation_id !== currentGraph) {
    changes.push(`代码图谱从 ${briefVersion(wiki.graph_generation_id)} 重建成了 ${briefVersion(currentGraph)}`);
  }
  return {
    kind: 'warn',
    message: changes.length
      ? `${generatedPart}，之后${changes.join('、')}，内容可能不是最新的。`
      : `${generatedPart}，之后源码或代码图谱有变动，内容可能不是最新的。`,
  };
}

function setRepoWikiNotice(message = '', kind = 'info', action = false) {
  const notice = document.getElementById('repowiki-notice');
  notice.replaceChildren();
  if (!message) return;
  const banner = document.createElement('div');
  banner.className = `banner ${kind}`;
  const span = document.createElement('span');
  span.textContent = message;
  banner.appendChild(span);
  if (action) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'btn ghost';
    button.dataset.repowikiOpenSettings = 'true';
    button.textContent = '打开设置';
    banner.appendChild(button);
  }
  notice.appendChild(banner);
}

function renderRepoWikiNoProjects() {
  state.repowiki.requestSerial += 1;
  resetRepoWikiSummary();
  resetRepoWikiNavigation();
  setRepoWikiNotice('当前 registry 没有可选择的共享或本机私有项目。', 'info');
  renderRepoWikiPageNav({});
  document.getElementById('repowiki-article').innerHTML = emptyStateHtml(
    '没有已登记项目',
    '请先在「项目管理」登记项目；这里不会接受或拼接任意本地路径。'
  );
  document.getElementById('repowiki-graph-results').innerHTML = emptyStateHtml(
    '没有可查询项目',
    '登记项目后再显式搜索本地图谱。'
  );
}

function repoWikiErrorCode(error) {
  if (typeof error?.code === 'string') return error.code;
  const match = String(error || '').match(/\[([a-z0-9_-]+)\]/i);
  return match ? match[1] : '';
}

function repoWikiErrorMessage(error) {
  const code = repoWikiErrorCode(error);
  const known = {
    repowiki_cli_path_missing: '尚未配置 RepoWiki CLI 绝对路径。',
    repowiki_data_root_missing: '尚未配置 RepoWiki data root 绝对路径。',
    invalid_repowiki_cli_path: 'RepoWiki CLI 路径不存在、不安全或不可执行。',
    invalid_repowiki_data_root: 'RepoWiki data root 不是可用的绝对目录。',
    project_not_bound: '该共享项目没有当前机器的本地路径绑定。',
    project_not_found: 'Dashboard registry 中不存在该项目。',
    repowiki_timeout: 'RepoWiki 只读命令执行超时。',
    repowiki_output_too_large: 'RepoWiki 返回内容超过安全上限。',
    repowiki_protocol_violation: 'RepoWiki CLI 返回的协议不兼容或无效。',
    repowiki_platform_unsupported: '当前 RepoWiki 只读浏览仅支持 macOS / Linux。',
    repository_not_initialized: '该项目尚未初始化 RepoWiki。',
    wiki_baseline_missing: '该项目尚未生成并发布正式 Wiki。',
    wiki_baseline_invalid: '正式 Wiki 基线无效或已损坏，导航暂不可读。',
    invalid_wiki_navigation: 'Wiki 导航数据无效或无法解析。',
    graph_missing: '当前分支没有本地图谱。',
    graph_stale: '本地图谱与当前源码不一致，请在终端显式更新后再查询。',
    graph_integrity_failed: '本地图谱已损坏，请在终端显式重建。',
  };
  return known[code] || '读取 RepoWiki 失败；请检查本机配置和项目状态。';
}

function isRepoWikiConfigError(error) {
  return new Set([
    'repowiki_cli_path_missing',
    'repowiki_data_root_missing',
    'invalid_repowiki_cli_path',
    'invalid_repowiki_data_root',
    'local_config_missing',
  ]).has(repoWikiErrorCode(error));
}

// 图谱类错误码统一 graph_ 前缀（graph_missing / graph_stale / graph_integrity_failed 等）。
// 单独识别是为了让图查询失败只染图谱区，不拖垮 wiki 浏览与 status 卡片。
function isRepoWikiGraphError(error) {
  return repoWikiErrorCode(error).startsWith('graph_');
}

function unwrapRepoWikiEnvelope(envelope, expectedCommand) {
  if (!envelope || typeof envelope !== 'object' || envelope.protocol_version !== 1) {
    throw Object.assign(new Error('[repowiki_protocol_violation] RepoWiki 协议版本无效'), { code: 'repowiki_protocol_violation' });
  }
  if (envelope.command !== expectedCommand || typeof envelope.ok !== 'boolean') {
    throw Object.assign(new Error('[repowiki_protocol_violation] RepoWiki 命令响应不匹配'), { code: 'repowiki_protocol_violation' });
  }
  if (!envelope.ok) {
    const remote = envelope.error && typeof envelope.error === 'object' ? envelope.error : {};
    const code = typeof remote.code === 'string' ? remote.code : 'repowiki_remote_error';
    const message = typeof remote.message === 'string' ? remote.message : 'RepoWiki 只读命令失败';
    throw Object.assign(new Error(`[${code}] ${message}`), { code });
  }
  return envelope.result && typeof envelope.result === 'object' ? envelope.result : {};
}

function repoWikiWarnings(envelope) {
  return Array.isArray(envelope?.warnings)
    ? envelope.warnings.filter(item => typeof item === 'string' && item.trim()).slice(0, 5)
    : [];
}

async function loadRepoWikiProject() {
  const selector = selectedRepoWikiSelector();
  if (!selector) {
    renderRepoWikiNoProjects();
    return;
  }
  const requestSerial = ++state.repowiki.requestSerial;
  state.repowiki.graphRequestSerial += 1;
  state.repowiki.status = null;
  state.repowiki.project = null;
  state.repowiki.generations = null;
  state.repowiki.currentPageId = '';
  resetRepoWikiSummary();
  resetRepoWikiNavigation();
  setRepoWikiNotice('正在只读检查当前项目…', 'info');
  renderRepoWikiPageNav({});
  document.getElementById('repowiki-article').innerHTML = emptyStateHtml('正在读取', '正在检查正式 Wiki 状态与导航。');
  document.getElementById('repowiki-graph-results').innerHTML = emptyStateHtml('尚未查询', '图查询只在提交搜索或点击节点操作时执行。');

  try {
    const statusEnvelope = await invoke('repowiki_status', { selector });
    const status = unwrapRepoWikiEnvelope(statusEnvelope, 'status');
    if (requestSerial !== state.repowiki.requestSerial) return;
    state.repowiki.status = status;
    renderRepoWikiOverview();

    if (status.initialization !== 'initialized' || status.wiki?.freshness === 'absent') {
      setRepoWikiNotice(
        status.initialization !== 'initialized'
          ? '该项目还没有初始化 RepoWiki。'
          : '该项目还没有生成正式 Wiki。',
        'info'
      );
      const article = document.getElementById('repowiki-article');
      article.innerHTML = emptyStateHtml(
        'Wiki 尚未生成',
        '请在项目终端显式生成并发布 RepoWiki，完成后点击「重新读取」。'
      );
      return;
    }

    const [projectSettled, generationsSettled] = await Promise.allSettled([
      invoke('repowiki_read_project', { selector }),
      invoke('repowiki_read_generations', { selector }),
    ]);
    if (requestSerial !== state.repowiki.requestSerial) return;
    const warnings = [...repoWikiWarnings(statusEnvelope)];
    // 导航（read_project）与代次（read_generations）解包各自兜底：
    // 导航失败留 projectError 走"只染导航+文章区"降级；代次是补充信息，失败直接忽略。
    let projectError = null;
    if (projectSettled.status === 'fulfilled') {
      try {
        state.repowiki.project = unwrapRepoWikiEnvelope(projectSettled.value, 'read project');
        warnings.push(...repoWikiWarnings(projectSettled.value));
      } catch (err) {
        projectError = err;
      }
    } else {
      projectError = projectSettled.reason;
    }
    if (generationsSettled.status === 'fulfilled') {
      try {
        state.repowiki.generations = unwrapRepoWikiEnvelope(generationsSettled.value, 'read generations');
        warnings.push(...repoWikiWarnings(generationsSettled.value));
      } catch (_err) {
        // 代次读取失败不影响主流程，忽略。
      }
    }
    renderRepoWikiOverview();

    if (!state.repowiki.project) {
      // 降级关键点：Wiki 导航读失败只染导航 + 文章区，绝不 throw、不进整页 failure；
      // status 卡片与图谱查询区保持独立可用（renderRepoWikiFailure 内部按错误码分级）。
      const reason = projectError
        || Object.assign(new Error('[invalid_wiki_navigation] 无法读取正式 Wiki 导航'), { code: 'invalid_wiki_navigation' });
      renderRepoWikiFailure(reason);
      return;
    }
    const notice = repoWikiStatusNotice(status, state.repowiki.generations);
    if (warnings.length) setRepoWikiNotice(`RepoWiki 提示：${warnings.join('；')}`, 'warn');
    else if (notice) setRepoWikiNotice(notice.message, notice.kind);
    else setRepoWikiNotice('当前内容来自 RepoWiki 正式发布区；Dashboard 只读展示。', 'info');

    document.getElementById('repowiki-article').innerHTML = emptyStateHtml(
      '请选择文档页面',
      '页面正文只在你点击左侧导航后读取。'
    );
  } catch (error) {
    if (requestSerial !== state.repowiki.requestSerial) return;
    renderRepoWikiFailure(error);
  }
}

function renderRepoWikiFailure(error) {
  // 失败分级渲染（降级核心）：
  //  - 图谱类（graph_*）→ 只染图谱区，wiki 浏览与 status 卡片不受影响；
  //  - 配置类（repowiki_*_missing / invalid_repowiki_* / local_config_missing）→ 整页提示去设置；
  //  - 其余（wiki 类 wiki_baseline_* / invalid_wiki_navigation，或项目状态类）→ 只染导航 + 文章区，
  //    status 卡片与图谱查询区保持独立可用，避免"wiki 一坏整页挂"。
  const code = repoWikiErrorCode(error);
  if (isRepoWikiGraphError(error)) {
    const results = document.getElementById('repowiki-graph-results');
    results.innerHTML = emptyStateHtml('图谱查询失败', repoWikiErrorMessage(error));
    return;
  }
  if (isRepoWikiConfigError(error)) {
    setRepoWikiNotice(repoWikiErrorMessage(error), 'err', true);
    resetRepoWikiNavigation();
    renderRepoWikiPageNav({});
    document.getElementById('repowiki-article').innerHTML = emptyStateHtml(
      'RepoWiki 未配置',
      '请在设置页保存 CLI 与 data root 的绝对路径；保存前不会写入任何配置。'
    );
    return;
  }
  // wiki 类 / 项目状态类：只染导航 + 文章区。
  setRepoWikiNotice(`Wiki 导航暂不可读：${code || 'unknown'}`, 'warn');
  resetRepoWikiNavigation();
  renderRepoWikiPageNav({});
  document.getElementById('repowiki-article').innerHTML = emptyStateHtml(
    'Wiki 导航暂不可读',
    repoWikiErrorMessage(error)
  );
}

// 清空当前项目的动态目录集合（切项目 / 读取中 / 降级时用）。
function resetRepoWikiNavigation() {
  state.repowiki.pages = [];
  state.repowiki.pageIndex = new Map();
  state.repowiki.pageIds = new Set();
  state.repowiki.pathIndex = new Map();
  state.repowiki.homePageId = '';
  state.repowiki.expandedPageIds = new Set();
}

// 校验并载入 v2 navigation（扁平数组 + parent_page_id/order 表达树形）。
// 逐项校验：page_id 必须是非空 string（否则丢弃该页）；title 缺失回退为 page_id；path 缺失回退空串。
// 结果写入 state.repowiki（pages/pageIndex/pageIds/homePageId/expandedPageIds），返回有序扁平数组供渲染。
function validatedRepoWikiNavigation(project) {
  const navigation = project?.navigation;
  const received = Array.isArray(navigation?.pages) ? navigation.pages : [];
  const pageIndex = new Map();
  const pages = [];
  for (const raw of received) {
    if (!raw || typeof raw !== 'object') continue;
    const pageId = typeof raw.page_id === 'string' ? raw.page_id.trim() : '';
    if (!pageId || pageIndex.has(pageId)) continue; // page_id 非法或重复 → 丢弃
    const title = typeof raw.title === 'string' && raw.title.trim() ? raw.title.trim() : pageId;
    const path = typeof raw.path === 'string' ? raw.path : '';
    const parentId = typeof raw.parent_page_id === 'string' && raw.parent_page_id.trim()
      ? raw.parent_page_id.trim()
      : null; // home 页 parent_page_id 为 null
    const order = Number.isFinite(Number(raw.order)) ? Number(raw.order) : Number.MAX_SAFE_INTEGER;
    const pageType = typeof raw.page_type === 'string' ? raw.page_type : '';
    const page = { page_id: pageId, title, path, page_type: pageType, parent_page_id: parentId, order };
    pageIndex.set(pageId, page);
    pages.push(page);
  }
  // home 判定：优先 navigation.home_page_id（须在集合内），否则取第一个"根"页（parent 为 null 或指向缺失父）。
  const isRoot = (page) => page.parent_page_id === null
    || !pageIndex.has(page.parent_page_id)
    || page.parent_page_id === page.page_id;
  let homePageId = typeof navigation?.home_page_id === 'string' && pageIndex.has(navigation.home_page_id.trim())
    ? navigation.home_page_id.trim()
    : '';
  if (!homePageId) {
    const rootPage = pages.find(isRoot);
    homePageId = rootPage ? rootPage.page_id : (pages[0]?.page_id || '');
  }
  state.repowiki.pages = pages;
  state.repowiki.pageIndex = pageIndex;
  state.repowiki.pageIds = new Set(pageIndex.keys());
  // path → page_id 反查表：v2 正文里的站内链接写的是相对 .md 路径，靠它把链接落回页面。
  const pathIndex = new Map();
  for (const page of pages) {
    const key = normalizeRepoWikiPath(page.path);
    if (key && !pathIndex.has(key)) pathIndex.set(key, page.page_id);
  }
  state.repowiki.pathIndex = pathIndex;
  state.repowiki.homePageId = homePageId;
  // 默认只展开第一层：展开所有根节点（露出其直接子页），更深层折叠，避免一次铺 741 个 DOM。
  state.repowiki.expandedPageIds = new Set(pages.filter(isRoot).map(page => page.page_id));
  return pages;
}

function shortRepoWikiId(value) {
  const text = typeof value === 'string' ? value : '';
  return text.length > 28 ? `${text.slice(0, 14)}…${text.slice(-10)}` : (text || '—');
}

function branchLabel(value) {
  return String(value || '—').replace(/^refs\/heads\//, '');
}

function repoWikiGeneratedTime(value) {
  if (typeof value !== 'string' || !value.trim()) return '生成时间：当前只读契约未提供';
  const time = new Date(value);
  return Number.isNaN(time.getTime())
    ? '生成时间：当前只读契约未提供'
    : `生成时间：${time.toLocaleString('zh-CN', { hour12: false })}`;
}

function graphStateLabel(graph) {
  if (!graph || graph.availability === 'absent') return '未生成';
  if (graph.integrity === 'damaged') return '已损坏';
  if (graph.source_freshness === 'stale') return '已过期';
  if (graph.availability === 'ready' && graph.source_freshness === 'current') return '可用 · 最新';
  return `${graph.availability || '未知'} · ${graph.source_freshness || '未知'}`;
}

function wikiStateLabel(wiki) {
  if (!wiki || wiki.freshness === 'absent') return '未生成';
  if (wiki.integrity === 'damaged' || wiki.freshness === 'damaged') return '已损坏';
  if (wiki.freshness === 'stale') return '已过期';
  if (wiki.freshness === 'current' && wiki.integrity === 'healthy') return '最新 · 完整';
  return `${wiki.freshness || '未知'} · ${wiki.integrity || '未知'}`;
}

// v2 代次只记「生成当时的 HEAD 与图谱代次」，新鲜度得跟 status 的当前值比才知道：
// 源码 HEAD 变了 → 已过期；图谱代次变了（含分析器 ABI 升级后的确定性重建）→ 同样已过期。
function repoWikiV2Freshness(wiki, status) {
  if (wiki.status !== 'complete') return 'damaged';
  const currentHead = status?.repository?.head_commit || status?.graph?.current_source_commit;
  if (currentHead && wiki.head_commit && currentHead !== wiki.head_commit) return 'stale';
  const currentGraph = status?.graph?.generation_id;
  if (currentGraph && wiki.graph_generation_id && currentGraph !== wiki.graph_generation_id) return 'stale';
  return 'current';
}

function renderRepoWikiOverview() {
  const status = state.repowiki.status || {};
  const project = state.repowiki.project || {};
  const generations = state.repowiki.generations || {};
  const branch = generations.branch?.branch_ref
    || project.repository?.branch_ref
    || status.repository?.branch_ref;
  const wikiGeneration = generations.wiki?.generation_id
    || project.wiki?.generation_id
    || status.wiki?.generation_id;
  const graph = generations.graph || status.graph || {};
  // 拿到 v2 直读代次就以它为准：status.wiki 走 CLI 的 v1 校验器，对 v2 wiki 恒报
  // damaged / 代次 null。拿不到（非 v2、或代次文件缺失）才回落 status.wiki。
  const generationWiki = generations.wiki?.generation_id ? generations.wiki : null;
  const wiki = resolveRepoWikiWikiState(status, generations);
  const generatedAt = generations.wiki?.generated_at
    || project.wiki?.generated_at
    || status.wiki?.generated_at;
  // 审查态同理：status.review 在 v2 下是 v1 固定 10 页的伪造清单（恒 stale），不能用。
  const review = (generationWiki ? generations.review : null) || status.review || {};
  const totals = review.totals || {};

  document.getElementById('repowiki-branch').textContent = branchLabel(branch);
  const generationEl = document.getElementById('repowiki-generation');
  generationEl.textContent = shortRepoWikiId(wikiGeneration);
  generationEl.title = wikiGeneration || '';
  document.getElementById('repowiki-wiki-state').textContent = `状态：${wikiStateLabel(wiki)}`;
  document.getElementById('repowiki-generation-time').textContent = repoWikiGeneratedTime(generatedAt);
  document.getElementById('repowiki-graph-state').textContent = graphStateLabel(graph);
  const graphGenerationEl = document.getElementById('repowiki-graph-generation');
  graphGenerationEl.textContent = `代次：${shortRepoWikiId(graph.generation_id)}`;
  graphGenerationEl.title = graph.generation_id || '';
  document.getElementById('repowiki-review-total').textContent = Number.isFinite(Number(totals.total))
    ? `${Number(totals.reviewed) || 0} / ${Number(totals.total) || 0} 已审`
    : '—';
  document.getElementById('repowiki-review-detail').textContent = Number.isFinite(Number(totals.total))
    ? `未审 ${Number(totals.unreviewed) || 0} · 过期 ${Number(totals.stale) || 0}`
    : '—';
  // 载入/清空当前项目动态目录（project 为空时清空），再树形渲染导航。
  validatedRepoWikiNavigation(state.repowiki.project);
  renderRepoWikiPageNav(review.pages || {});

  const graphResults = document.getElementById('repowiki-graph-results');
  if (graph.availability === 'absent') {
    graphResults.innerHTML = emptyStateHtml('当前分支没有本地图谱', 'Dashboard 不会自动构建；请在终端显式执行图谱构建后再读取。');
  } else if (graph.integrity === 'damaged') {
    graphResults.innerHTML = emptyStateHtml('本地图谱已损坏', '请在终端显式完整重建；Dashboard 不会自动修复。');
  } else if (graph.source_freshness === 'stale') {
    graphResults.innerHTML = emptyStateHtml('本地图谱已过期', '请在终端显式增量更新；Dashboard 不会自动更新。');
  }
}

// 树形渲染导航：按 parent_page_id 建父→子映射，每层按 order 升序（title 兜底）；
// 从根（home，parent 为 null）递归；只把已展开子树写进 DOM，避免一次铺 741 个节点。
function renderRepoWikiPageNav(reviewPages = {}) {
  const nav = document.getElementById('repowiki-page-nav');
  nav.replaceChildren();
  const pages = Array.isArray(state.repowiki.pages) ? state.repowiki.pages : [];
  if (!pages.length) return; // 无导航数据：留空，提示交给 notice / 文章区
  const index = state.repowiki.pageIndex;
  const childrenById = new Map();
  const roots = [];
  for (const page of pages) {
    const parentId = (page.parent_page_id && index.has(page.parent_page_id) && page.parent_page_id !== page.page_id)
      ? page.parent_page_id
      : null;
    if (parentId === null) { roots.push(page); continue; }
    if (!childrenById.has(parentId)) childrenById.set(parentId, []);
    childrenById.get(parentId).push(page);
  }
  const byOrder = (a, b) => (a.order - b.order) || String(a.title).localeCompare(String(b.title), 'zh-CN');
  roots.sort(byOrder);
  for (const list of childrenById.values()) list.sort(byOrder);

  const expanded = state.repowiki.expandedPageIds;
  const visited = new Set(); // 环保护：畸形 parent 链不至于无限递归
  const renderLevel = (nodes, depth) => {
    for (const page of nodes) {
      if (visited.has(page.page_id)) continue;
      visited.add(page.page_id);
      const children = childrenById.get(page.page_id) || [];
      const hasChildren = children.length > 0;
      const isExpanded = hasChildren && expanded.has(page.page_id);
      nav.appendChild(buildRepoWikiNavRow(page, depth, hasChildren, isExpanded, reviewPages));
      if (isExpanded) renderLevel(children, depth + 1);
    }
  };
  renderLevel(roots, 0);
}

// 单行导航：折叠开关（有子节点时）+ 页面按钮（含审查状态点）。子级缩进随 depth 递增。
function buildRepoWikiNavRow(page, depth, hasChildren, isExpanded, reviewPages) {
  const row = document.createElement('div');
  row.className = 'repowiki-nav-row';
  row.style.paddingLeft = (depth * 14) + 'px';

  if (hasChildren) {
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'repowiki-nav-toggle';
    toggle.dataset.togglePageId = page.page_id;
    toggle.setAttribute('aria-expanded', isExpanded ? 'true' : 'false');
    toggle.setAttribute('aria-label', isExpanded ? '收起' : '展开');
    toggle.textContent = isExpanded ? '▾' : '▸';
    row.appendChild(toggle);
  } else {
    // 叶子占位，保持与有开关行的图标列对齐。
    const spacer = document.createElement('span');
    spacer.className = 'repowiki-nav-toggle-spacer';
    spacer.setAttribute('aria-hidden', 'true');
    row.appendChild(spacer);
  }

  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'repowiki-nav-item';
  button.dataset.pageId = page.page_id;
  button.classList.toggle('active', page.page_id === state.repowiki.currentPageId);
  const title = document.createElement('span');
  title.className = 'repowiki-nav-title';
  title.textContent = page.title;
  const review = document.createElement('span');
  // v2 无 per-page review 数据时缺省 unknown（显示 '—'），不依赖固定页集、不报错。
  const reviewState = ['reviewed', 'unreviewed', 'stale'].includes(reviewPages?.[page.page_id])
    ? reviewPages[page.page_id]
    : 'unknown';
  review.className = `repowiki-review-dot ${reviewState}`;
  review.textContent = {
    reviewed: '已审', unreviewed: '未审', stale: '过期', unknown: '—'
  }[reviewState];
  button.append(title, review);
  row.appendChild(button);
  return row;
}

// 展开/收起某个父节点，仅改折叠状态后重渲染（不触发任何 IPC）。
function toggleRepoWikiNavNode(pageId) {
  if (!state.repowiki.pageIds.has(pageId)) return;
  const expanded = state.repowiki.expandedPageIds;
  if (expanded.has(pageId)) expanded.delete(pageId);
  else expanded.add(pageId);
  renderRepoWikiPageNav(state.repowiki.status?.review?.pages || {});
}

// 剥掉页面 md 开头一行 <!-- repowiki:page-meta {...} --> 注释，避免其被渲染进正文。
// v2 正文里散布 CLI 写入的内部标记注释：首行 page-meta，以及 source-baseline /
// evidence-index / change-ledger 的 start|end 成对标记。正文渲染会转义 HTML，注释因此
// 变成肉眼可见的文本，故整行剥掉标记本身；标记「之间」的内容（源码基线说明、证据索引
// 链接）是要展示的正文，保留不动。
function stripRepoWikiPageMeta(content) {
  return String(content ?? '').replace(/^[ \t]*<!--\s*repowiki:[\s\S]*?-->[ \t]*\r?\n?/gm, '');
}

async function loadRepoWikiPage(pageId, expectedProjectSerial = state.repowiki.requestSerial) {
  // 白名单以当前项目动态 page_id 集合为准（v2 动态目录）。
  if (!state.repowiki.pageIds.has(pageId)) return;
  const selector = selectedRepoWikiSelector();
  if (!selector) return;
  state.repowiki.currentPageId = pageId;
  renderRepoWikiPageNav(state.repowiki.status?.review?.pages || {});
  const article = document.getElementById('repowiki-article');
  article.innerHTML = emptyStateHtml('正在读取页面', '正文将由 RepoWiki 校验正式代次和内容哈希后返回。');
  try {
    const envelope = await invoke('repowiki_read_page', { selector, pageId });
    const page = unwrapRepoWikiEnvelope(envelope, 'read page');
    if (expectedProjectSerial !== state.repowiki.requestSerial || state.repowiki.currentPageId !== pageId) return;
    if (page.page_id !== pageId || typeof page.content !== 'string') {
      throw Object.assign(new Error('[repowiki_protocol_violation] Wiki 页面响应不匹配'), { code: 'repowiki_protocol_violation' });
    }
    const content = stripRepoWikiPageMeta(page.content);
    article.innerHTML = `<header class="repowiki-article-head">
      <div><h1>${escapeHtml(page.title || page.page_id)}</h1><code>${escapeHtml(page.page_id)}</code></div>
      <span class="badge info" title="${escapeHtml(page.wiki_generation_id || '')}">代次 ${escapeHtml(shortRepoWikiId(page.wiki_generation_id))}</span>
    </header>
    <div class="repowiki-markdown">${renderSafeWikiMarkdown(content)}</div>`;
  } catch (error) {
    if (expectedProjectSerial !== state.repowiki.requestSerial || state.repowiki.currentPageId !== pageId) return;
    article.innerHTML = emptyStateHtml('页面读取失败', repoWikiErrorMessage(error));
  }
  // Wiki 正文与概览共用主区滚动；切页后定位新正文，旧项目/页面响应不能移动当前视图。
  if (state.activeTab === 'repowiki' && expectedProjectSerial === state.repowiki.requestSerial &&
      state.repowiki.currentPageId === pageId) {
    const main = document.getElementById('main');
    main.scrollTop += article.getBoundingClientRect().top - main.getBoundingClientRect().top - 16;
  }
}

function stripBlockedWikiHtml(markdown) {
  let value = String(markdown ?? '');
  for (const tag of ['script', 'style', 'iframe', 'object', 'svg', 'math']) {
    const block = new RegExp(`<\\s*${tag}\\b[^>]*>[\\s\\S]*?<\\s*\\/\\s*${tag}\\s*>`, 'gi');
    const loose = new RegExp(`<\\s*\\/?\\s*${tag}\\b[^>]*>`, 'gi');
    value = value.replace(block, '').replace(loose, '');
  }
  return value.replace(/<\s*img\b[^>]*>/gi, '');
}

// 规范化相对 `.repowiki/` 的路径：吃掉 `.`、按段消解 `..`；越过根返回空串（视为站外）。
function normalizeRepoWikiPath(value) {
  const parts = [];
  for (const segment of String(value || '').split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      if (!parts.length) return '';
      parts.pop();
      continue;
    }
    parts.push(segment);
  }
  return parts.join('/');
}

// 把正文链接解析成可跳转的 page_id；解析不出（站外链接 / 源码文件 / 未登记页）返回 null。
// v2 正文的站内链接是**相对当前页的 .md 路径**（可能 URL 编码、可能带 #锚点），
// 与 v1 的 `repowiki:<id>` / `#<id>` 写法并存，故两种都认。
// currentPath 省略时取当前正在阅读的页面路径；显式传入便于单测覆盖跨层级的 `../` 解析。
function repoWikiPageIdFromLink(target, currentPath) {
  const value = String(target || '').trim();
  if (!value) return null;
  if (value.startsWith('repowiki:')) {
    const candidate = value.slice('repowiki:'.length);
    return state.repowiki.pageIds.has(candidate) ? candidate : null;
  }
  if (value.startsWith('#')) {
    const candidate = value.slice(1);
    return state.repowiki.pageIds.has(candidate) ? candidate : null;
  }
  // 带协议的一律不可点（http/mailto/javascript 等）。
  if (/^[a-z][a-z0-9+.-]*:/i.test(value)) return null;
  const withoutAnchor = value.split('#')[0];
  if (!withoutAnchor) return null;
  let decoded = withoutAnchor;
  try {
    decoded = decodeURIComponent(withoutAnchor);
  } catch (_error) {
    // 非法转义序列：按原样继续解析，解析不出就是不可点。
  }
  const basePath = typeof currentPath === 'string'
    ? currentPath
    : (state.repowiki.pageIndex.get(state.repowiki.currentPageId)?.path || '');
  // 以 `/` 开头视为相对 `.repowiki/` 根，否则相对当前页所在目录。
  const baseDir = decoded.startsWith('/')
    ? ''
    : basePath.slice(0, basePath.lastIndexOf('/') + 1);
  const resolved = normalizeRepoWikiPath(baseDir + decoded.replace(/^\/+/, ''));
  const pageId = resolved ? state.repowiki.pathIndex.get(resolved) : '';
  return pageId && state.repowiki.pageIds.has(pageId) ? pageId : null;
}

function renderWikiInline(text) {
  const tokens = [];
  const token = html => {
    const index = tokens.push(html) - 1;
    return `\uE000RW${index}\uE001`;
  };
  let value = String(text ?? '');
  value = value.replace(/`([^`\n]+)`/g, (_match, code) => token(`<code>${escapeHtml(code)}</code>`));
  value = value.replace(/!\[([^\]\n]*)\]\(([^)\n]*)\)/g, (_match, alt) => token(`<span class="wiki-blocked-image">${escapeHtml(alt || '[图片已移除]')}</span>`));
  value = value.replace(/\[([^\]\n]+)\]\(([^)\n]+)\)/g, (_match, label, target) => {
    const pageId = repoWikiPageIdFromLink(target);
    return pageId
      ? token(`<button type="button" class="wiki-page-link" data-page-id="${escapeHtml(pageId)}">${escapeHtml(label)}</button>`)
      // 非站内页面（源码文件 / 站外地址）：只做不可点的引用样式。不画删除线——那会让
      // "指向源码的正常引用"看起来像失效内容。链接目标一律不落进 DOM（既有安全约束：
      // 渲染结果不得出现任何链接地址），故提示语固定、不带 target。
      : token(`<span class="wiki-external-ref" title="该链接指向 Wiki 之外，不可在此打开">${escapeHtml(label)}</span>`);
  });
  value = escapeHtml(value)
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/\*([^*\n]+)\*/g, '<em>$1</em>');
  // \u5FC5\u987B\u9012\u5F52\u8FD8\u539F\uFF1A\u5360\u4F4D\u7B26\u4F1A**\u5D4C\u5957**\u3002\u8BC1\u636E\u7D22\u5F15\u90A3\u79CD [`\u7B26\u53F7\u540D`](\u8DEF\u5F84) \u7684\u5199\u6CD5\u91CC\uFF0C\u884C\u5185\u4EE3\u7801\u5148\u88AB\u62BD\u6210
  // \u5360\u4F4D\u7B26\uFF0C\u94FE\u63A5\u6807\u7B7E\u4E8E\u662F\u53D8\u6210\u5360\u4F4D\u7B26\u672C\u8EAB\uFF0C\u518D\u88AB\u62BD\u6210\u5916\u5C42 token\uFF1B\u53EA\u8FD8\u539F\u4E00\u5C42\u7684\u8BDD\uFF0C\u6807\u7B7E\u4F4D\u7F6E\u4F1A\u628A
  // `\uE000RW0\uE001` \u539F\u6837\u5410\u7ED9\u7528\u6237\uFF08\u6E32\u67D3\u6210\u4E24\u4E2A\u8C46\u8150\u5757\u5939\u7740 RW0\uFF09\uFF0C\u7B26\u53F7\u540D\u6574\u4E2A\u4E22\u5931\u3002
  // token \u53EA\u4F1A\u5F15\u7528\u6BD4\u81EA\u5DF1\u5E8F\u53F7\u5C0F\u7684 token\uFF0C\u6545\u9012\u5F52\u5FC5\u7136\u6536\u655B\u3002
  const expand = (input) => input.replace(
    /\uE000RW(\d+)\uE001/g,
    (_match, index) => expand(tokens[Number(index)] || ''),
  );
  return expand(value);
}

// 同页多张图时 marker id 不能重名，故用自增序号。
let repoWikiDiagramSerial = 0;

// 渲染 mermaid 的 graph / flowchart 子集（RepoWiki 生成的就是这一种有向图）。
// 只认「节点 --> 节点」「节点 -.->|标签| 节点」这类行；出现子集外的语法（时序图、
// 类图、子图等）一律返回空串，由调用方回落成代码块——宁可显示源码，也不吞内容或画错。
function renderMermaidFlowchart(source) {
  const rows = String(source || '')
    .split('\n')
    .map(line => line.trim())
    .filter(line => line && !line.startsWith('%%'));
  if (rows.length < 2) return '';
  const header = rows[0].match(/^(?:graph|flowchart)\s+(TD|TB|LR|RL)$/i);
  if (!header) return '';
  const horizontal = /^(LR|RL)$/i.test(header[1]);

  const nodes = new Map();
  const edges = [];
  const ensureNode = (id, label) => {
    const existing = nodes.get(id);
    if (!existing) nodes.set(id, { id, label: label || id });
    else if (label) existing.label = label;
  };
  const NODE = String.raw`([A-Za-z0-9_.-]+)(?:[\[\(\{]\s*"?([^"\]\)\}]*)"?\s*[\]\)\}])?`;
  const CONNECTOR = String.raw`(-{2,}>|-\.-*>|={2,}>|-{3,})`;
  const EDGE_LABEL = String.raw`(?:\|\s*"?([^"|]*)"?\s*\|\s*)?`;
  const edgePattern = new RegExp(`^${NODE}\\s*${CONNECTOR}\\s*${EDGE_LABEL}${NODE};?$`);
  const nodeOnlyPattern = new RegExp(`^${NODE};?$`);
  for (const row of rows.slice(1)) {
    const edge = row.match(edgePattern);
    if (edge) {
      const [, fromId, fromLabel, connector, edgeLabel, toId, toLabel] = edge;
      ensureNode(fromId, fromLabel);
      ensureNode(toId, toLabel);
      edges.push({ from: fromId, to: toId, dashed: connector.includes('.'), label: (edgeLabel || '').trim() });
      continue;
    }
    const only = row.match(nodeOnlyPattern);
    if (only && only[2] !== undefined) {
      ensureNode(only[1], only[2]);
      continue;
    }
    return ''; // 子集外语法：整体回落
  }
  if (!nodes.size || !edges.length) return '';

  // 文本宽度按字符类别估算：CJK 约一个字宽，ASCII 约半个字宽。
  const textWidth = (text) => [...String(text)]
    .reduce((sum, char) => sum + (/[⺀-鿿　-〿＀-￯]/.test(char) ? 13 : 7.2), 0);
  const LINE_HEIGHT = 18;
  const PAD_X = 14;
  const PAD_Y = 12;
  const GAP_MAIN = 62;  // 层间距（含边标签空间）
  const GAP_CROSS = 26; // 同层间距
  for (const node of nodes.values()) {
    node.lines = String(node.label).split(/<br\s*\/?>/i).map(line => line.trim()).filter(Boolean);
    if (!node.lines.length) node.lines = [node.id];
    node.width = Math.max(...node.lines.map(textWidth)) + PAD_X * 2;
    node.height = node.lines.length * LINE_HEIGHT + PAD_Y * 2;
  }

  // 分层：沿边做有界松弛取最长路径深度；有环时被迭代次数截断，不会死循环。
  const depth = new Map([...nodes.keys()].map(id => [id, 0]));
  for (let round = 0; round < nodes.size; round += 1) {
    let changed = false;
    for (const edge of edges) {
      const next = depth.get(edge.from) + 1;
      if (next > depth.get(edge.to)) { depth.set(edge.to, next); changed = true; }
    }
    if (!changed) break;
  }
  const layers = [];
  for (const node of nodes.values()) {
    const level = depth.get(node.id) || 0;
    (layers[level] || (layers[level] = [])).push(node);
  }

  // 主轴 = 层推进方向，交叉轴 = 同层排开方向；LR/RL 只是把两轴对调。
  const layerExtent = layers.map(layer => Math.max(...layer.map(n => (horizontal ? n.width : n.height))));
  const crossExtent = layers.map(layer =>
    layer.reduce((sum, n) => sum + (horizontal ? n.height : n.width), 0) + GAP_CROSS * (layer.length - 1));
  const crossTotal = Math.max(...crossExtent);
  let mainCursor = 0;
  layers.forEach((layer, level) => {
    let crossCursor = (crossTotal - crossExtent[level]) / 2;
    for (const node of layer) {
      const crossSize = horizontal ? node.height : node.width;
      if (horizontal) { node.x = mainCursor; node.y = crossCursor; }
      else { node.x = crossCursor; node.y = mainCursor; }
      crossCursor += crossSize + GAP_CROSS;
    }
    mainCursor += layerExtent[level] + GAP_MAIN;
  });
  const mainTotal = mainCursor - GAP_MAIN;
  const width = Math.ceil((horizontal ? mainTotal : crossTotal)) + 2;
  const height = Math.ceil((horizontal ? crossTotal : mainTotal)) + 2;

  const marker = `rw-arrow-${repoWikiDiagramSerial += 1}`;
  const edgeParts = edges.map(edge => {
    const from = nodes.get(edge.from);
    const to = nodes.get(edge.to);
    // 起终点取相对方向的边中点，主轴方向连线。
    const forward = horizontal ? to.x >= from.x : to.y >= from.y;
    const start = horizontal
      ? { x: forward ? from.x + from.width : from.x, y: from.y + from.height / 2 }
      : { x: from.x + from.width / 2, y: forward ? from.y + from.height : from.y };
    const end = horizontal
      ? { x: forward ? to.x : to.x + to.width, y: to.y + to.height / 2 }
      : { x: to.x + to.width / 2, y: forward ? to.y : to.y + to.height };
    const line = `<path d="M${start.x.toFixed(1)},${start.y.toFixed(1)} L${end.x.toFixed(1)},${end.y.toFixed(1)}"`
      + ` fill="none" stroke="currentColor" stroke-width="1.4"${edge.dashed ? ' stroke-dasharray="5 4"' : ''}`
      + ` marker-end="url(#${marker})" opacity="0.75"/>`;
    if (!edge.label) return line;
    const midX = (start.x + end.x) / 2;
    const midY = (start.y + end.y) / 2;
    const labelWidth = textWidth(edge.label) + 10;
    return `${line}<rect class="rw-diagram-label-bg" x="${(midX - labelWidth / 2).toFixed(1)}" y="${(midY - 9).toFixed(1)}"`
      + ` width="${labelWidth.toFixed(1)}" height="18" rx="4"/>`
      + `<text class="rw-diagram-edge-label" x="${midX.toFixed(1)}" y="${(midY + 4).toFixed(1)}"`
      + ` text-anchor="middle">${escapeHtml(edge.label)}</text>`;
  });

  const nodeParts = [...nodes.values()].map(node => {
    const centerX = node.x + node.width / 2;
    const firstY = node.y + PAD_Y + LINE_HEIGHT - 5;
    const tspans = node.lines.map((line, index) =>
      `<tspan x="${centerX.toFixed(1)}" y="${(firstY + index * LINE_HEIGHT).toFixed(1)}">${escapeHtml(line)}</tspan>`
    ).join('');
    return `<rect class="rw-diagram-node" x="${node.x.toFixed(1)}" y="${node.y.toFixed(1)}"`
      + ` width="${node.width.toFixed(1)}" height="${node.height.toFixed(1)}" rx="7"/>`
      + `<text class="rw-diagram-node-text" text-anchor="middle">${tspans}</text>`;
  });

  return `<div class="repowiki-diagram"><svg viewBox="0 0 ${width} ${height}" width="${width}" height="${height}"`
    + ` role="img" aria-label="流程图"><defs><marker id="${marker}" viewBox="0 0 10 10" refX="9" refY="5"`
    + ` markerWidth="7" markerHeight="7" orient="auto-start-reverse">`
    + `<path d="M0,0 L10,5 L0,10 z" fill="currentColor"/></marker></defs>`
    + `${edgeParts.join('')}${nodeParts.join('')}</svg></div>`;
}

function renderSafeWikiMarkdown(markdown) {
  const lines = stripBlockedWikiHtml(markdown).replace(/\r\n?/g, '\n').split('\n');
  const out = [];
  let paragraph = [];
  let listKind = '';
  let listItems = [];
  let inFence = false;
  let fenceLang = '';
  let codeLines = [];
  let tableLines = [];
  // mermaid 围栏优先尝试画成图；画不出（子集外语法）回落成代码块。
  const flushFence = () => {
    const source = codeLines.join('\n');
    codeLines = [];
    const diagram = fenceLang === 'mermaid' ? renderMermaidFlowchart(source) : '';
    fenceLang = '';
    out.push(diagram || `<pre><code>${escapeHtml(source)}</code></pre>`);
  };
  const flushParagraph = () => {
    if (!paragraph.length) return;
    out.push(`<p>${paragraph.map(renderWikiInline).join('<br>')}</p>`);
    paragraph = [];
  };
  const flushList = () => {
    if (!listItems.length) return;
    out.push(`<${listKind}>${listItems.map(item => `<li>${renderWikiInline(item)}</li>`).join('')}</${listKind}>`);
    listKind = '';
    listItems = [];
  };
  // 表格：第二行必须是 `| --- | --- |` 形式的分隔行才算合法 GFM 表格；
  // 不合法就按普通段落原样吐回，绝不吞内容。
  const splitTableCells = (line) => {
    let text = line.trim();
    if (text.startsWith('|')) text = text.slice(1);
    if (text.endsWith('|') && !text.endsWith('\\|')) text = text.slice(0, -1);
    const cells = [];
    let current = '';
    for (let index = 0; index < text.length; index += 1) {
      const character = text[index];
      if (character === '\\' && text[index + 1] === '|') { current += '|'; index += 1; continue; }
      if (character === '|') { cells.push(current.trim()); current = ''; continue; }
      current += character;
    }
    cells.push(current.trim());
    return cells;
  };
  const isTableSeparator = (line) => /^\s*\|?[\s:|-]+\|?\s*$/.test(line) && line.includes('-');
  const flushTable = () => {
    if (!tableLines.length) return;
    const rows = tableLines;
    tableLines = [];
    if (rows.length < 2 || !isTableSeparator(rows[1])) {
      out.push(`<p>${rows.map(renderWikiInline).join('<br>')}</p>`);
      return;
    }
    const head = splitTableCells(rows[0]);
    const body = rows.slice(2).map(splitTableCells);
    out.push(`<table class="repowiki-table"><thead><tr>${
      head.map(cell => `<th>${renderWikiInline(cell)}</th>`).join('')
    }</tr></thead><tbody>${
      body.map(row => `<tr>${row.map(cell => `<td>${renderWikiInline(cell)}</td>`).join('')}</tr>`).join('')
    }</tbody></table>`);
  };
  const flushBlocks = () => { flushParagraph(); flushList(); flushTable(); };

  for (const line of lines) {
    if (/^\s*```/.test(line)) {
      if (inFence) {
        flushFence();
        inFence = false;
      } else {
        flushBlocks();
        inFence = true;
        fenceLang = line.trim().replace(/^`+/, '').trim().toLowerCase();
      }
      continue;
    }
    if (inFence) {
      codeLines.push(line);
      continue;
    }
    if (!line.trim()) {
      flushBlocks();
      continue;
    }
    const heading = line.match(/^(#{1,4})\s+(.+)$/);
    if (heading) {
      flushBlocks();
      const level = heading[1].length;
      out.push(`<h${level}>${renderWikiInline(heading[2])}</h${level}>`);
      continue;
    }
    if (/^\s*(?:---+|___+)\s*$/.test(line)) {
      flushBlocks();
      out.push('<hr>');
      continue;
    }
    const unordered = line.match(/^\s*[-+*]\s+(.+)$/);
    const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
    if (unordered || ordered) {
      flushParagraph();
      flushTable();
      const nextKind = unordered ? 'ul' : 'ol';
      if (listKind && listKind !== nextKind) flushList();
      listKind = nextKind;
      listItems.push((unordered || ordered)[1]);
      continue;
    }
    const quote = line.match(/^\s*>\s?(.*)$/);
    if (quote) {
      flushBlocks();
      out.push(`<blockquote>${renderWikiInline(quote[1])}</blockquote>`);
      continue;
    }
    if (/^\s*\|.*\|\s*$/.test(line)) {
      flushParagraph();
      flushList();
      tableLines.push(line);
      continue;
    }
    flushList();
    flushTable();
    paragraph.push(line);
  }
  if (inFence) flushFence();
  flushBlocks();
  return out.join('\n');
}

async function searchRepoWikiGraph() {
  const selector = selectedRepoWikiSelector();
  const input = document.getElementById('repowiki-graph-query');
  const text = (input.value || '').trim();
  if (!selector) {
    document.getElementById('repowiki-graph-results').innerHTML = emptyStateHtml('没有可查询项目', '请先选择已登记项目。');
    return;
  }
  if (!text) {
    input.focus();
    return;
  }
  const serial = state.repowiki.requestSerial;
  const graphSerial = ++state.repowiki.graphRequestSerial;
  const results = document.getElementById('repowiki-graph-results');
  results.innerHTML = emptyStateHtml('正在搜索', '正在执行有界、只读的固定图查询。');
  try {
    const envelope = await invoke('repowiki_graph_search', { selector, text, limit: 20 });
    const result = unwrapRepoWikiEnvelope(envelope, 'graph search');
    if (serial !== state.repowiki.requestSerial || graphSerial !== state.repowiki.graphRequestSerial) return;
    renderRepoWikiGraphResult(result, 'search', repoWikiWarnings(envelope));
  } catch (error) {
    if (serial !== state.repowiki.requestSerial || graphSerial !== state.repowiki.graphRequestSerial) return;
    results.innerHTML = emptyStateHtml('图搜索失败', repoWikiErrorMessage(error));
  }
}

async function runRepoWikiGraphOperation(action, nodeId) {
  const commandMap = {
    show: ['repowiki_graph_show', 'graph show'],
    callers: ['repowiki_graph_callers', 'graph callers'],
    callees: ['repowiki_graph_callees', 'graph callees'],
    impact: ['repowiki_graph_impact', 'graph impact'],
  };
  const command = commandMap[action];
  const selector = selectedRepoWikiSelector();
  if (!command || !selector || !REPOWIKI_NODE_ID.test(String(nodeId || ''))) return;
  const serial = state.repowiki.requestSerial;
  const graphSerial = ++state.repowiki.graphRequestSerial;
  const results = document.getElementById('repowiki-graph-results');
  results.innerHTML = emptyStateHtml('正在查询关系', '正在执行固定、只读且有深度/数量上限的图查询。');
  const args = action === 'show'
    ? { selector, nodeId }
    : { selector, nodeId, depth: 2, limit: 50 };
  try {
    const envelope = await invoke(command[0], args);
    const result = unwrapRepoWikiEnvelope(envelope, command[1]);
    if (serial !== state.repowiki.requestSerial || graphSerial !== state.repowiki.graphRequestSerial) return;
    renderRepoWikiGraphResult(result, action, repoWikiWarnings(envelope));
  } catch (error) {
    if (serial !== state.repowiki.requestSerial || graphSerial !== state.repowiki.graphRequestSerial) return;
    results.innerHTML = emptyStateHtml('关系查询失败', repoWikiErrorMessage(error));
  }
}

function renderRepoWikiGraphNode(node) {
  if (!node || typeof node !== 'object') return '';
  const nodeId = typeof node.node_id === 'string' && REPOWIKI_NODE_ID.test(node.node_id)
    ? node.node_id
    : '';
  const name = node.qualified_name || node.name || '(未命名节点)';
  const location = [node.path || '', Number.isFinite(Number(node.line)) ? `:${Number(node.line)}` : ''].join('');
  const actions = nodeId ? `<div class="repowiki-node-actions">
    <button type="button" class="btn ghost" data-graph-action="show" data-node-id="${nodeId}">详情</button>
    <button type="button" class="btn ghost" data-graph-action="callers" data-node-id="${nodeId}">调用者</button>
    <button type="button" class="btn ghost" data-graph-action="callees" data-node-id="${nodeId}">被调用者</button>
    <button type="button" class="btn ghost" data-graph-action="impact" data-node-id="${nodeId}">影响面</button>
  </div>` : '';
  return `<div class="repowiki-node-card">
    <div class="repowiki-node-main">
      <strong>${escapeHtml(name)}</strong>
      <span class="badge info">${escapeHtml(node.kind || 'node')}</span>
      ${node.hop != null ? `<span class="badge mute">${escapeHtml(node.hop)} 跳</span>` : ''}
    </div>
    <code title="${escapeHtml(location)}">${escapeHtml(location || '未知位置')}</code>
    ${actions}
  </div>`;
}

function renderRepoWikiEdges(edges) {
  if (!Array.isArray(edges) || !edges.length) return '';
  const rows = edges.slice(0, 100).map(edge => `<li>
    <code>${escapeHtml(shortRepoWikiId(edge?.source_id))}</code>
    <span>${escapeHtml(edge?.type || 'RELATION')}</span>
    <code>${escapeHtml(shortRepoWikiId(edge?.target_id))}</code>
  </li>`).join('');
  return `<details class="repowiki-edges"><summary>关系边 ${edges.length} 条</summary><ul>${rows}</ul></details>`;
}

function renderRepoWikiGraphResult(result, operation, warnings = []) {
  const results = document.getElementById('repowiki-graph-results');
  const nodes = operation === 'show'
    ? (result?.node ? [result.node] : [])
    : (Array.isArray(result?.nodes) ? result.nodes : []);
  const edges = Array.isArray(result?.edges) ? result.edges : [];
  if (!nodes.length && !edges.length) {
    results.innerHTML = emptyStateHtml('没有匹配结果', '换一个更精确的函数名、类名或项目相对路径再试。');
    return;
  }
  const labels = { search: '搜索结果', show: '节点详情', callers: '调用者', callees: '被调用者', impact: '影响面' };
  results.innerHTML = `<div class="repowiki-result-head">
      <strong>${escapeHtml(labels[operation] || '图查询结果')}</strong>
      <span>${nodes.length} 个节点${result?.truncated ? ' · 已按上限截断' : ''}</span>
    </div>
    ${warnings.length ? `<div class="banner warn">${escapeHtml(warnings.join('；'))}</div>` : ''}
    <div class="repowiki-node-list">${nodes.map(renderRepoWikiGraphNode).join('')}</div>
    ${renderRepoWikiEdges(edges)}`;
}

async function refreshAggregateSilently() {
  try {
    state.aggregate = await invoke('aggregate_fleet_board', {});
    state.aggregateError = null;
    renderFleetBoard();
  } catch (e) {
    // 静默失败：不打扰用户但**必须让用户看得见**（对账 #6）。
    // 保留旧 aggregate 数据 + 顶部 banner 提示"读取失败，当前为上次数据"。
    console.warn('aggregate silent refresh', e);
    state.aggregateError = String(e);
    renderFleetBoard();
  }
}

// -------- Sync bar --------
function bindSyncButtons() {
  document.getElementById('btn-refresh').addEventListener('click', refreshAll);
  document.getElementById('btn-sync').addEventListener('click', manualSync);
}

async function manualSync() {
  if (state.syncInProgress) return;
  state.syncInProgress = true;
  ++state.syncStatusRequestSerial; // 使同步之前发出的历史读取失效。
  state.syncRecordWarning = null;
  state.syncStatus.error = null;
  const btn = document.getElementById('btn-sync');
  btn.disabled = true;
  const status = document.getElementById('sync-status');
  status.classList.remove('error', 'stale');
  status.textContent = '同步中…';
  // 决策 6：手动同步是全库采集，可能耗时十几秒；超时阈值 15s（原 5s 会误判长采集卡死）。
  // P2：watchdog 覆盖「invoke + refreshAll」全程——invoke 返回后 refreshAll 仍可能挂起，
  // 提前 clearTimeout 会让"同步中…"永久卡住且无卡死提示，故 clearTimeout 统一放 finally。
  const timeoutId = setTimeout(() => {
    // 决策 6 / 复核 P1-4：15s 无反馈 → 顶栏红色告警 + 「重启 App」按钮（addEventListener 不用 inline
    // onclick，避 Tauri v2 严格 CSP）。重启动作 = 重载 webview：恢复"refreshAll 异步挂起 / 前端卡住"
    // 这类可恢复场景；真·后端主线程死锁 JS 无解，用户需手动 Cmd+Q 重启进程（真·进程 relaunch 待接 process 插件）。
    status.classList.add('error');
    status.replaceChildren();
    const warn = document.createElement('span');
    warn.textContent = 'App 可能卡死，请重启 ';
    const restartBtn = document.createElement('button');
    restartBtn.type = 'button';
    restartBtn.className = 'btn ghost';
    restartBtn.textContent = '重启 App';
    restartBtn.addEventListener('click', () => { window.location.reload(); });
    status.append(warn, restartBtn);
  }, 15000);
  try {
    const r = await invoke('manual_sync', {});
    // 决策 6：按判别式 outcome 更新持久 state，交 updateSyncStatus 统一渲染（notice 优先 last）。
    // 绝不无条件写 last（顶栏假成功根治）；非成功结局落 state.syncStatus.notice 而非只改 DOM——
    // 落 state 后，后续任意 refreshAll 才不会用旧 last 把失败态复活成假成功（P1）。
    const outcome = (r && r.outcome) || {};
    if (outcome.status === 'synced' && outcome.pushed === true) {
      state.syncStatus.last = new Date(r.collected_at);
      state.syncStatus.notice = null; // 真出机成功：清除旧的未配置 / 推送失败提示
    } else if (outcome.status === 'unconfigured') {
      state.syncStatus.notice = '未配置存储仓，未触发同步';
    } else if (outcome.status === 'synced' && outcome.pushed === false) {
      state.syncStatus.notice = '已本地保存·推送失败·下次重试';
    }
    state.syncRecordWarning = r?.sync_record_warning || null;
    // 先恢复后端实际完成时间；记录落盘失败时保留本次响应，不能用旧记录覆盖警告。
    if (!state.syncRecordWarning) await refreshSyncStatus(true);
    await refreshAll();
  } catch (e) {
    state.syncStatus.error = '跨机同步失败：' + e;
  } finally {
    clearTimeout(timeoutId); // P2：invoke + refreshAll 全程走完才清 watchdog
    state.syncInProgress = false;
    btn.disabled = false;
    updateSyncStatus();
  }
}

// 纯函数（决策 6 · P1）：据同步状态算顶栏文案 + 是否 stale 样式，隔离 DOM 便于单测。
// notice（未配置 / 推送失败等"非成功结局"）优先于 last——否则后续任意 refreshAll 会用旧 last
// 把失败态复活成「最后同步 X 分钟前」假成功。notice 由 manualSync 按 outcome 落 state。
function computeSyncStatusView(syncStatus, nowMs) {
  if (syncStatus?.error) return { text: syncStatus.error, stale: false, error: true };
  if (syncStatus && syncStatus.notice) {
    return { text: syncStatus.notice, stale: true };
  }
  if (!syncStatus || !syncStatus.last) {
    return { text: '跨机同步：暂无同步记录', stale: false };
  }
  const time = syncStatus.last.getTime();
  if (!Number.isFinite(time)) return { text: '同步记录时间无效，请重新读取', stale: false, error: true };
  const ago = Math.max(0, Math.round((nowMs - time) / 60000));
  if (ago > 360) {
    // 决策 5：6h 自动周期已删，去「周期触发上限」措辞，只保留"数据偏旧、建议手动同步"的陈旧提示
    return { text: `上次跨机同步成功：${ago} 分钟前 · 数据偏旧，建议手动同步`, stale: true };
  }
  return { text: `上次跨机同步成功：${ago} 分钟前`, stale: false };
}

async function refreshSyncStatus(duringSync = false) {
  if ((state.syncInProgress && !duringSync) || state.syncRecordWarning) return;
  const serial = ++state.syncStatusRequestSerial;
  try {
    const record = await invoke('get_sync_status', {});
    if (serial !== state.syncStatusRequestSerial) return;
    const next = { last: record?.last_success_at ? new Date(record.last_success_at) : null, error: null, notice: null };
    switch (record?.last_attempt?.outcome) {
      case 'push-failed': next.notice = '已本地保存·推送失败·下次重试'; break;
      case 'unconfigured': next.notice = '未配置存储仓，未触发同步'; break;
      case 'failed': next.error = '上次跨机同步失败，请重试'; break;
      case 'succeeded': break;
      default:
        // 当前操作已给出失败/未配置时，缺失的历史不能清除本次确定的结果。
        if (duringSync && (state.syncStatus.error || state.syncStatus.notice)) return;
    }
    state.syncStatus = next;
  } catch (e) {
    if (serial === state.syncStatusRequestSerial) state.syncStatus.error = '同步记录读取失败：' + e;
  }
}

function updateSyncStatus() {
  if (state.syncInProgress) return; // 普通刷新不能覆盖同步中/超时提示。
  const status = document.getElementById('sync-status');
  status.classList.remove('error', 'stale');
  const view = computeSyncStatusView(state.syncStatus, Date.now());
  if (view.stale) status.classList.add('stale');
  if (view.error) status.classList.add('error');
  if (state.syncRecordWarning) status.classList.add('stale');
  status.textContent = view.text + (state.syncRecordWarning ? ` · ${state.syncRecordWarning}` : '');
}

// -------- Refresh --------
async function refreshAll() {
  const syncRead = refreshSyncStatus();
  const sessionView = activeSessionView();
  await fetchSessions(sessionView.page, sessionView.pageSize);
  try {
    state.projects = await invoke('list_projects', {});
    state.projectsLoaded = true; // 对账 #3：加载完成才允许 Fleet Board 空态分场景判断
    renderProjects();
  } catch (e) {
    console.error('list_projects', e);
    // 拉取失败也标 loaded，让 Fleet 显示 empty state (a) 而不是永久 loading
    state.projectsLoaded = true;
  }
  try {
    state.aggregate = await invoke('aggregate_fleet_board', {});
    state.aggregateError = null;
    renderFleetBoard();
  } catch (e) {
    console.error('aggregate', e);
    state.aggregateError = String(e);
    renderFleetBoard();
  }
  await syncRead;
  updateSyncStatus();
}

// 拉分页 session 数据。query 从搜索框实时读；page/pageSize 显式传（切页 / 页大小切换 / 搜索都会调用）
async function fetchSessions(page, pageSize) {
  const source = state.sessionSource;
  const view = state.sessionViews[source];
  view.query = document.getElementById('session-query').value || '';
  const query = view.query.trim() || null;
  view.page = page;
  view.pageSize = pageSize;
  view.notice = '';
  const visibility = view.visibility;
  const requestSerial = ++state.sessionListRequestSerial[source];
  view.loading = true;
  view.error = '';
  if (state.sessionSource === source && view.items.length === 0) renderSessions();
  try {
    const resp = await invoke('list_sessions', { source, query, page, pageSize, visibility });
    if (requestSerial !== state.sessionListRequestSerial[source]) return;
    view.items = resp.items || [];
    view.page = resp.page;
    view.pageSize = resp.page_size;
    view.total = resp.total;
    view.needsRefresh = false;
    view.error = '';
    view.loading = false;
    if (state.sessionSource === source) {
      document.getElementById('session-page-size').value = String(view.pageSize);
      renderSessions();
      renderPagination();
    }
    return true;
  } catch (e) {
    if (requestSerial !== state.sessionListRequestSerial[source]) return;
    view.error = String(e);
    view.loading = false;
    if (state.sessionSource === source) {
      renderSessions();
      renderPagination();
    }
    console.error('fetchSessions', e);
    return false;
  }
}

// -------- Session Board --------
function renderSessions() {
  // 后端已按 query 过滤 + 分页 slice，前端不再二次过滤
  const view = activeSessionView();
  const notice = document.getElementById('session-notice');
  if (notice) { notice.textContent = view.notice; notice.classList.toggle('hidden', !view.notice); }
  const tbody = document.querySelector('#session-table tbody');
  // 重绘只接续当前仍在本表内的焦点；异步返回不能抢走用户已移到搜索框等处的焦点。
  const focused = document.activeElement;
  const focusKey = tbody.contains(focused) && (focused?.dataset.sessionId
    ? { selector: '.btn-favorite', id: focused.dataset.sessionId, key: 'sessionId' }
    : focused?.dataset.visibilityId
      ? { selector: '.btn-session-visibility', id: focused.dataset.visibilityId, key: 'visibilityId' }
      : null);
  tbody.innerHTML = '';
  for (const s of view.items) {
    const tr = document.createElement('tr');
    // title 有值正常显示；纯 UUID（cc 未生成 ai-title）时灰色 + [无标题] 前缀
    const hasTitle = s.title && s.title !== s.session_id;
    const titleCell = hasTitle
      ? escapeHtml(s.title)
      : `<span style="color:var(--text-mute);">[无标题] ${escapeHtml(s.session_id.slice(0, 8))}</span>`;

    // 图标独占一个布局盒；以可访问状态控制实心/空心，避免伪元素与旧字形排成两行。
    const favoriteLabel = s.favorited ? '取消收藏' : '收藏';
    const favoriteBtn = `<button type="button" class="btn-favorite" data-session-id="${escapeHtml(s.session_id)}" title="${favoriteLabel}" aria-label="${favoriteLabel}" aria-pressed="${s.favorited}" aria-disabled="${view.pendingFavorites.has(s.session_id)}"><svg class="favorite-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m12 2 3 6.5 7 1-5 5 1.2 7L12 18l-6.2 3.5 1.2-7-5-5 7-1z"/></svg></button>`;

    const hideLabel = view.visibility === 'hidden' ? '恢复显示' : '隐藏';
    const hideIcon = view.visibility === 'hidden' ? 'show' : 'hide';
    const hideBtn = `<button type="button" class="btn-session-visibility" data-visibility-id="${escapeHtml(s.session_id)}" title="${hideLabel}" aria-label="${hideLabel}" aria-disabled="${view.pendingHidden.has(s.session_id)}"><span class="session-visibility-icon ${hideIcon}" aria-hidden="true"></span></button>`;
    tr.innerHTML = `
      <td style="text-align:center;"><span class="session-marks">${favoriteBtn}${hideBtn}</span></td>
      <td>${escapeHtml(relTime(s.mtime))}</td>
      <td style="color:var(--text-mute);font-variant-numeric:tabular-nums;">${escapeHtml(absTime(s.mtime))}</td>
      <td>${escapeHtml(s.projects || '')}</td>
      <td>${titleCell}</td>
      <td style="color:var(--text-mute)">${escapeHtml(s.cwd || '')}</td>
      <td><code>${escapeHtml(s.session_id.slice(0, 8))}</code></td>
    `;

    const source = state.sessionSource;
    tr.querySelector('.btn-favorite').addEventListener('click', (event) => {
      event.stopPropagation();
      toggleSessionFavorite(source, s.session_id);
    });
    tr.querySelector('.btn-session-visibility').addEventListener('click', (event) => {
      event.stopPropagation();
      requestSessionHidden(source, s, view.visibility !== 'hidden');
    });

    tr.addEventListener('click', () => { selectRow(tr); showSessionDetail(s); });
    tbody.appendChild(tr);
  }
  if (view.loading && view.items.length === 0) {
    tbody.innerHTML = `<tr><td colspan="7" style="text-align:center;color:var(--text-mute);">（${escapeHtml(sessionSourceLabel(state.sessionSource))} · 读取中…）</td></tr>`;
  } else if (view.error) {
    tbody.innerHTML = `<tr><td colspan="7" style="text-align:center;color:var(--danger);">读取 ${escapeHtml(sessionSourceLabel(state.sessionSource))} 会话失败：${escapeHtml(view.error)}</td></tr>`;
  } else if (view.items.length === 0) {
    const suffix = view.query.trim() ? '无匹配会话' : view.visibility === 'hidden' ? '没有已隐藏会话' : '暂无活动会话，可切换到“已隐藏”查看';
    tbody.innerHTML = `<tr><td colspan="7" style="text-align:center;color:var(--text-mute);">（${escapeHtml(sessionSourceLabel(state.sessionSource))} · ${suffix}）</td></tr>`;
  }
  if (focusKey && state.activeTab === 'sessions') {
    const next = [...tbody.querySelectorAll(focusKey.selector)].find(el => el.dataset[focusKey.key] === focusKey.id);
    (next || document.getElementById('session-visibility')).focus({ preventScroll: true });
  }
}

function renderPagination() {
  const { page, pageSize, total } = activeSessionView();
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const start = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, total);
  document.getElementById('session-pagination-info').textContent =
    total === 0 ? '共 0 条' : `第 ${start}-${end} 条 / 共 ${total} 条`;

  const pages = document.getElementById('session-pagination-pages');
  pages.innerHTML = '';
  const btn = (label, targetPage, disabled = false, active = false) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'page-btn' + (active ? ' active' : '') + (disabled ? ' disabled' : '');
    b.textContent = label;
    if (!disabled && !active) {
      b.addEventListener('click', () => fetchSessions(targetPage, pageSize));
    }
    return b;
  };
  pages.appendChild(btn('«', 1, page === 1));
  pages.appendChild(btn('‹', page - 1, page === 1));
  // 显示当前页 ±2 的窗口，边界自动收敛（保证最多 5 个页码按钮）
  let startPage = Math.max(1, page - 2);
  let endPage = Math.min(totalPages, startPage + 4);
  startPage = Math.max(1, endPage - 4);
  for (let i = startPage; i <= endPage; i++) {
    pages.appendChild(btn(String(i), i, false, i === page));
  }
  pages.appendChild(btn('›', page + 1, page === totalPages));
  pages.appendChild(btn('»', totalPages, page === totalPages));
}

async function showSessionDetail(session) {
  const source = session.source || state.sessionSource;
  const detailKey = `${source}:${session.session_id}`;
  const requestSerial = ++state.sessionDetailRequestSerial;
  state.sessionDetailKey = detailKey;
  const drawer = document.getElementById('drawer');
  document.getElementById('drawer-title').textContent = session.title || session.session_id;
  const extras = document.getElementById('drawer-extras');
  const body = document.getElementById('drawer-body');
  // extras 区域固定不滚：完整 session-id + 复制按钮
  extras.innerHTML = `
    <div class="form-row" style="margin-bottom:8px;">
      <label>完整 session-id</label>
      <code style="user-select:all;">${escapeHtml(session.session_id)}</code>
    </div>
    <div class="toolbar" style="margin:0;">
      <button class="btn primary" data-copy-source="${escapeHtml(source)}"
              data-copy-sid="${escapeHtml(session.session_id)}">复制 resume 命令</button>
    </div>
  `;
  body.innerHTML = '<div class="dialog">加载中...</div>';
  drawer.classList.add('open');
  drawer.setAttribute('aria-hidden', 'false');
  document.body.classList.add('drawer-open');
  // 复制按钮先绑定，不必等待详情解析完成。
  extras.querySelectorAll('[data-copy-sid]').forEach(btn => {
    btn.addEventListener('click', () => copyResumeCmd(
      btn.getAttribute('data-copy-source'),
      btn.getAttribute('data-copy-sid'),
    ));
  });
  try {
    const detail = await invoke('session_detail', { source, sessionId: session.session_id });
    if (requestSerial !== state.sessionDetailRequestSerial || detailKey !== state.sessionDetailKey) return;
    body.innerHTML = `
      <div class="form-row">
        <label>最近 5 轮对话（1 轮 = 1 user + 1 assistant，取 tail 512KB 范围内）</label>
        <div class="msg-list">${renderDialogMarkdown(detail?.preview || [])}</div>
      </div>
      <div class="form-row">
        <label>改过的文件</label>
        <div class="dialog">${(detail?.files || []).map(f => escapeHtml(f)).join('\n')}</div>
      </div>
    `;
  } catch (e) {
    if (requestSerial !== state.sessionDetailRequestSerial || detailKey !== state.sessionDetailKey) return;
    body.innerHTML = `<div class="banner err">加载详情失败: ${escapeHtml(String(e))}</div>`;
  }
}

function closeDrawer() {
  state.sessionDetailRequestSerial += 1;
  state.sessionDetailKey = '';
  const d = document.getElementById('drawer');
  d.classList.remove('open');
  d.setAttribute('aria-hidden', 'true');
  document.body.classList.remove('drawer-open');
  clearRowSelection(); // 关抽屉时清掉表格行高亮，保持"选中即抽屉打开"一致
  // 关闭时清空 extras 段，避免下次打开残留上一次的 session-id / resume 按钮
  const extras = document.getElementById('drawer-extras');
  if (extras) extras.innerHTML = '';
}

// v4.3.1：表格行选中高亮 —— 点击行打开详情时高亮当前行（先清同 tbody 其他选中），
// 关抽屉 / 切 tab 统一由 closeDrawer → clearRowSelection 清除。Fleet 表 + Session 表共用。
function selectRow(tr) {
  const tbody = tr.closest('tbody');
  if (tbody) tbody.querySelectorAll('tr.selected').forEach(r => r.classList.remove('selected'));
  tr.classList.add('selected');
}

function clearRowSelection() {
  document.querySelectorAll('table.list tr.selected').forEach(r => r.classList.remove('selected'));
}

async function copyResumeCmd(source, sid) {
  try {
    const cmd = await invoke('copy_resume_command', { source, sessionId: sid });
    await navigator.clipboard.writeText(cmd);
    flash('已复制：' + cmd);
  } catch (e) { flash('复制失败：' + e, true); }
}

// -------- Fleet Board --------

// Lucide-风格 SVG icon 内联渲染（spec `Requirement: 视觉设计规则 · Scenario: 零 emoji 图标` 硬约束）。
// 前端 escapeHtml 已保护数据流，图标是静态字符串直接 concat 安全。
const ICON_ALERT_TRIANGLE = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>';
const ICON_ALERT_CIRCLE = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/></svg>';
// CHECK_CIRCLE 用于 info 态（synced / ahead / single-machine） —— 对账 #11：
// 之前 verdictBannerIcon 全用 ALERT_CIRCLE 让"同步成功"看着像有警告
const ICON_CHECK_CIRCLE = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>';
const ICON_INBOX = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/></svg>';
const ICON_PLUS_CIRCLE = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 8v8M8 12h8"/></svg>';

function renderFleetBoard() {
  const tbody = document.querySelector('#fleet-table tbody');
  const bans = document.getElementById('fleet-banners');
  const table = document.getElementById('fleet-table');
  bans.innerHTML = '';
  tbody.innerHTML = '';

  // aggregateError 顶部横幅（对账 #6）—— 静默刷新失败时用户看陈旧数据必须有提示
  if (state.aggregateError) {
    const div = document.createElement('div');
    div.className = 'banner err';
    div.innerHTML = `${ICON_ALERT_TRIANGLE}<span>读取跨机聚合失败：${escapeHtml(state.aggregateError)}（当前显示为上次成功读取的数据）</span>`;
    bans.appendChild(div);
  }

  // v4.2 §16.8：URL→UUID 冲突单独打头（多命中触发，spec 硬约束的红色告警）
  const conflicts = state.aggregate.filter(p => p.verdict.kind === 'uuid-conflict');
  for (const c of conflicts) {
    const div = document.createElement('div');
    div.className = 'banner err';
    // verdict.differing_urls 是 [[uuid, url], ...]（见 aggregate.rs Verdict::UuidConflict）
    const pairs = (c.verdict.differing_urls || [])
      .map(([uid, url]) => `${String(uid).slice(0, 8)}… → ${url}`)
      .join('；');
    div.innerHTML = `${ICON_ALERT_TRIANGLE}<span><strong>URL→UUID 冲突</strong>：项目"${escapeHtml(c.name)}" 命中多个 UUID（${escapeHtml(pairs)}）。请去<strong>项目管理页</strong>删除冗余 UUID 恢复全局唯一 invariant。</span>`;
    bans.appendChild(div);
  }
  // 其他告警项目：分叉 / 双 dirty → 红条横幅（沿用旧文案）
  const otherAlerts = state.aggregate.filter(p => ['diverged', 'dual-dirty'].includes(p.verdict.kind));
  for (const a of otherAlerts) {
    const div = document.createElement('div');
    div.className = 'banner err';
    div.innerHTML = `${ICON_ALERT_TRIANGLE}<span>${escapeHtml(a.name)}：${escapeHtml(verdictLabel(a.verdict))}</span>`;
    bans.appendChild(div);
  }
  // 陈旧提示（黄条，默认 24h 降色阈值）
  const stale = state.aggregate.filter(p => p.stale);
  if (stale.length > 0) {
    const div = document.createElement('div');
    div.className = 'banner warn';
    div.innerHTML = `${ICON_ALERT_CIRCLE}<span>${stale.length} 个项目的快照已超过 24 小时，请在对应机器采集并同步最新状态。</span>`;
    bans.appendChild(div);
  }

  // 空态：根据本机 registry 状态区分场景 (a) / (b) / loading，各自给出可执行下一步
  if (state.aggregate.length === 0) {
    table.style.display = 'none';
    renderFleetEmptyState(tbody.parentElement);
    return;
  }
  table.style.display = '';
  removeFleetEmptyState(tbody.parentElement);

  for (const agg of state.aggregate) {
    const tr = document.createElement('tr');
    if (agg.stale) tr.classList.add('stale');
    const branchChips = renderChipList(agg.machines, (v) => v.project.branch, { valFallback: '(未 checkout)' });
    const headChips = renderChipList(agg.machines, (v) => (v.project.head ? v.project.head.slice(0, 8) : null), { valFallback: '—' });
    // warnIf 改传 raw view 而非格式化字符串（对账 #8）—— 未来 formatDirty 改文案不会漏掉高亮
    const dirtyChips = renderChipList(agg.machines, (v) => formatDirty(v.project), {
      valFallback: '0',
      warnIf: (_val, view) => (Number(view.project.dirty) || 0) > 0 || (Number(view.project.untracked) || 0) > 0,
    });

    tr.innerHTML = `
      <td><strong>${escapeHtml(agg.name)}</strong></td>
      <td>${branchChips}</td>
      <td>${headChips}</td>
      <td>${dirtyChips}</td>
      <td>${verdictBadge(agg.verdict)}</td>
    `;
    tr.style.cursor = 'pointer';
    tr.addEventListener('click', () => { selectRow(tr); showAggregateDetail(agg); });
    tbody.appendChild(tr);
  }
}

// 把 dirty/untracked 两个计数合成一行文字：`已改 N · 未跟踪 M` 或"干净"。
// 用 Number() + Math.max 兜底非数字 / 位运算截断问题（对账 #7）
function formatDirty(p) {
  const d = Math.max(0, Number(p.dirty) || 0);
  const u = Math.max(0, Number(p.untracked) || 0);
  if (d === 0 && u === 0) return '干净';
  const parts = [];
  if (d > 0) parts.push(`已改 ${d}`);
  if (u > 0) parts.push(`未跟踪 ${u}`);
  return parts.join(' · ');
}

// 把 machines map 渲染成一列 chip（每机一行），列宽随内容自适应，长 hostname 不撑爆行。
// warnIf(val, view) 传原始 view（含 project + machine_updated_at 等），避免格式化字符串硬编码耦合。
function renderChipList(machines, extractVal, opts = {}) {
  const items = Object.entries(machines).map(([mid, v]) => {
    const raw = extractVal(v);
    const val = (raw == null || raw === '') ? (opts.valFallback || '—') : String(raw);
    const isWarn = opts.warnIf ? opts.warnIf(val, v) : false;
    return `<span class="chip ${isWarn ? 'warn' : ''}">
      <span class="chip-key" title="${escapeHtml(mid)}">${escapeHtml(mid)}</span>
      <span class="chip-val" title="${escapeHtml(val)}">${escapeHtml(val)}</span>
    </span>`;
  });
  return `<div class="chip-list">${items.join('')}</div>`;
}

// 判断某台机的 snapshot 是否陈旧（>= 24h）—— 前端自算 per-machine stale（对账 #4）。
// 后端 agg.stale 是"有任意一台机陈旧"的聚合布尔，不能反推每台机的具体状态。
function isMachineStale(machineUpdatedAt) {
  if (!machineUpdatedAt) return false;
  const t = new Date(machineUpdatedAt).getTime();
  if (isNaN(t)) return false;
  return (Date.now() - t) > STALE_THRESHOLD_MS;
}

// v4.3+：单条 commit 的文件级改动汇总成一行「N 文件 +x −y」（第一版只给汇总、文件级明细留到
// v2，防信息墙）。files == null 表示该条未采文件级（仅前 5 条采）→ 返回空串。
function commitFilesSummary(files) {
  if (files == null) return '';
  if (files.length === 0) return '<span class="mute">无文件改动</span>';
  let add = 0, del = 0;
  for (const f of files) {
    add += Number(f.additions) || 0; // 二进制文件 additions=null → 记 0
    del += Number(f.deletions) || 0;
  }
  return `${files.length} 文件 <span class="commit-add">+${add}</span> <span class="commit-del">−${del}</span>`;
}

// v4.3+：drilldown 里每台机的「近 5 次提交」。三态对齐后端 recent_commits：
// null/undefined=采集不可用（重新同步后可见）；[]=空仓；非空=列表（每条一行：短 sha + 标题 + 时间 + 文件汇总）。
function renderRecentCommits(p) {
  const rc = p.recent_commits;
  if (rc == null) return '<div class="commits-note mute">提交记录不可用（重新同步后可见）</div>';
  if (rc.length === 0) return '<div class="commits-note mute">暂无提交</div>';
  const rows = rc.slice(0, 5).map((c) => {
    const sha = escapeHtml((c.sha_full || '').slice(0, 7));
    const subj = escapeHtml(c.subject || '');
    const when = escapeHtml(c.time ? relTimeUtc(c.time) : '—');
    const summary = commitFilesSummary(c.files);
    const meta = summary ? `${when} · ${summary}` : when;
    return `<li class="commit-row"><code class="commit-sha">${sha}</code><span class="commit-subj" title="${subj}">${subj}</span><span class="commit-meta mute">${meta}</span></li>`;
  }).join('');
  return `<div class="commits-title">近 5 次提交</div><ul class="commit-list">${rows}</ul>`;
}

// 空态引导（三分支）：
// - LOADING：state.projectsLoaded === false → registry 尚未拉取完，展示 loading 骨架（对账 #3）
// - (a) shared 数组为空 → 用户没配 sync:true 项目，引导去项目管理页新增（切 tab 是纯 UI 动作，符合 spec 只读浏览）
// - (b) shared 非空但 aggregate 为空 → snapshot 还没数据，**引导到顶栏「手动同步」按钮**，
//      不在 Fleet Board 内直接触发同步（spec `Requirement: Fleet Board 只读浏览` 硬约束，对账 #5）
function renderFleetEmptyState(card) {
  removeFleetEmptyState(card);
  const wrap = document.createElement('div');
  wrap.className = 'empty-state';
  wrap.id = 'fleet-empty-state';

  if (!state.projectsLoaded) {
    // LOADING：refreshAll 还没跑完 list_projects，无法判断 (a)/(b)，先展示 loading
    wrap.innerHTML = `
      <div class="empty-icon">${ICON_INBOX}</div>
      <div class="empty-title">加载中…</div>
      <div class="empty-hint">正在从本地 registry 读取跨机项目配置</div>
    `;
    card.appendChild(wrap);
    return;
  }

  const noSharedProjects = !state.projects.shared || state.projects.shared.length === 0;
  if (noSharedProjects) {
    wrap.innerHTML = `
      <div class="empty-icon">${ICON_INBOX}</div>
      <div class="empty-title">还没有跨机同步的项目</div>
      <div class="empty-hint">
        Fleet Board 展示的是勾选了「跨机同步（sync: true）」的项目状态。<br>
        目前 registry 里一个都没有 —— 去项目管理页新增，或把已有的私有项目改为跨机同步。
      </div>
      <div class="empty-actions">
        <button type="button" class="btn primary" id="empty-go-projects">${ICON_PLUS_CIRCLE}<span>去项目管理页新增</span></button>
      </div>
    `;
  } else {
    // 不给 [立即手动同步] 按钮（对账 #5）—— Fleet Board 页保持只读浏览，
    // 用户去顶栏点手动同步。SVG 箭头视觉引导到顶栏方向。
    wrap.innerHTML = `
      <div class="empty-icon">${ICON_INBOX}</div>
      <div class="empty-title">已配置 ${state.projects.shared.length} 个跨机项目，但暂无 snapshot 数据</div>
      <div class="empty-hint">
        本机采集器还没跑过，或 dashboard-state 私有仓里还没收到 snapshot。<br>
        请点击右上角的<strong style="color:var(--brand-cyan);">「手动同步」</strong>按钮触发一次采集 + push + pull，
        snapshot 就位后 Fleet Board 会自动显示各机状态。
      </div>
    `;
  }
  card.appendChild(wrap);

  // addEventListener 而非 inline onclick —— tauri v2 严格 CSP 下 inline handler 可能不触发
  const goProj = document.getElementById('empty-go-projects');
  if (goProj) goProj.addEventListener('click', () => switchTab('projects'));
}

function removeFleetEmptyState(card) {
  const old = document.getElementById('fleet-empty-state');
  if (old) old.remove();
}

// v4.2 §16.8 TODO：`git_remote_mismatch` / `git_remote_failed` events 目前只落 events.ndjson，
// 未通过 `ProjectAggregate` 上浮到前端。等 aggregate 加载 events tail 并塞入 `MachineProjectView`
// 之后，可以在 renderChipList 里针对每台机加"remote 已变"黄 badge。当前用户可以打开 dashboard-state
// 仓库看 `machines/<id>/events.ndjson` 里 `git_remote_mismatch` 行诊断。M4 backlog。

// v4.3.1：Insufficient 按后端 reason 细分文案（对齐 aggregate.rs InsufficientReason，kebab-case 序列化）。
// 旧版一句"信息不足（HEAD 缺失 / 需 fetch）"套所有成因会误导 —— 尤其"需 fetch"从来不是本 App 的解法。
function insufficientLabel(reason) {
  switch (reason) {
    case 'head-missing': return '某台机快照过旧（缺完整 HEAD），请在该机重新同步';
    case 'head-mismatch': return 'HEAD 不同，请查看下方两机提交记录';
    case 'multi-machine': return '暂不支持 3+ 台机判定';
    case 'url-degraded': return 'git URL 无法归一化，跨机比对已降级';
    default:
      console.warn('[fleet] unknown insufficient reason:', reason);
      return '信息不足';
  }
}

function verdictLabel(v) {
  switch (v.kind) {
    case 'synced': return '同步';
    case 'one-in-progress': return `进行中（${v.machine_id}）`;
    case 'ahead': return `${v.latest_machine_id} 领先 ${v.behind_machine_id}`;
    case 'diverged': return '分叉：需手动合并';
    case 'dual-dirty': return '两台都有未提交改动';
    case 'uuid-conflict': return 'URL→UUID 冲突：请去项目管理页删除冗余 UUID';
    case 'single-machine': return `仅 ${v.machine_id}`;
    case 'insufficient': return insufficientLabel(v.reason);
    default:
      // 静默降级 = 前端 outdated 后用户只看到"信息不足"，无法区分是"真不足"还是"新枚举没识别"。
      // 显式打 warn + 让 badge 展示未知 kind，方便用户立刻抓到问题。
      console.warn('[fleet] unknown verdict kind:', v.kind, v);
      return `未知判定（${v.kind || 'null'}）`;
  }
}

function verdictBadge(v) {
  const label = verdictLabel(v);
  const klass = {
    'synced': 'ok',
    'ahead': 'info',
    'one-in-progress': 'warn',
    'diverged': 'err',
    'dual-dirty': 'err',
    'uuid-conflict': 'err',
    'single-machine': 'mute',
    'insufficient': 'mute',
  }[v.kind] || 'warn'; // 未知 kind 默认 warn，跟"信息不足"的 mute 区分开
  return `<span class="badge ${klass}">${escapeHtml(label)}</span>`;
}

// 相对时间：X 秒 / 分钟 / 小时 / 天前
function relTimeUtc(iso) {
  if (!iso) return '—';
  const t = new Date(iso).getTime();
  if (isNaN(t)) return '—';
  const ago = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (ago < 60) return `${ago} 秒前`;
  if (ago < 3600) return `${Math.round(ago / 60)} 分钟前`;
  if (ago < 86400) return `${Math.round(ago / 3600)} 小时前`;
  return `${Math.round(ago / 86400)} 天前`;
}

function showAggregateDetail(agg) {
  const drawer = document.getElementById('drawer');
  document.getElementById('drawer-title').textContent = agg.name;
  const extras = document.getElementById('drawer-extras');
  const body = document.getElementById('drawer-body');

  // extras 区固定：项目元数据（UUID / git URL / 判定）
  // v4.2 §16.8：UUID 权威在 dashboard-state registry；无 UUID = 单机私有项目（sync:false）
  const uuidLine = agg.project_id
    ? `<code style="user-select:all;">${escapeHtml(agg.project_id)}</code>`
    : '<span style="color:var(--text-mute);">(单机私有项目 sync:false)</span>';
  const gitUrl = agg.git_url ? escapeHtml(agg.git_url) : '<span style="color:var(--text-mute);">(registry 未提供)</span>';
  extras.innerHTML = `
    <div class="form-row" style="margin-bottom:8px;">
      <label>项目 UUID</label>
      ${uuidLine}
    </div>
    <div class="form-row" style="margin-bottom:8px;">
      <label>git URL（归一化）</label>
      <div class="dialog">${gitUrl}</div>
    </div>
    <div class="banner ${verdictBannerClass(agg.verdict)}">${verdictBannerIcon(agg.verdict)}<span>判定：${escapeHtml(verdictLabel(agg.verdict))}</span></div>
  `;

  // body 区滚动：各机 snapshot 分块 + events TODO 占位
  const machineBlocks = Object.entries(agg.machines).map(([mid, view]) => {
    const p = view.project;
    // per-machine stale（对账 #4）：前端自算，不用 agg.stale 全局降色
    const machineStale = isMachineStale(view.machine_updated_at);
    const staleClass = machineStale ? ' stale' : '';
    const collectedRel = relTimeUtc(p.collected_at);
    const lastCommitRel = p.last_commit_at ? relTimeUtc(p.last_commit_at) : '—';
    // upstream 走 escapeHtml —— 数据来自后端 git remote，不能假设安全（对账 #1 XSS 防护）
    const upstreamLine = p.upstream
      ? escapeHtml(p.upstream)
      : '<span class="mute">(无 upstream)</span>';
    const aheadBehind = (p.ahead != null || p.behind != null)
      ? `↑ ${Number(p.ahead) || 0} · ↓ ${Number(p.behind) || 0}`
      : '<span class="mute">—</span>';
    const dirtyText = formatDirty(p);
    const dirtyRaw = Math.max(0, Number(p.dirty) || 0) + Math.max(0, Number(p.untracked) || 0);
    const dirtyCls = dirtyRaw > 0 ? 'warn' : '';
    // dangerous_state 是 kebab-case string enum（tracker.rs: Rebase / Merge / CherryPick / Detached），
    // 直接展示字符串即可；escapeHtml 兜底防未来变复杂结构时 XSS
    const dangerLine = p.dangerous_state
      ? `<dt>危险态</dt><dd class="danger">${escapeHtml(typeof p.dangerous_state === 'string' ? p.dangerous_state : JSON.stringify(p.dangerous_state))}</dd>`
      : '';
    // progress 字段修正（对账 #2 硬 bug）：后端 tracker.rs::Progress { done, total, next }，
    // 不是 { summary }。原实现读 p.progress.summary 永远显示不出来
    let progressLine = '';
    if (p.progress) {
      const parts = [];
      if (p.progress.done != null && p.progress.total != null) {
        parts.push(`${Number(p.progress.done) || 0}/${Number(p.progress.total) || 0}`);
      }
      if (p.progress.next) parts.push(`next: ${p.progress.next}`);
      if (parts.length) {
        progressLine = `<dt>进度</dt><dd>${escapeHtml(parts.join(' · '))}</dd>`;
      }
    }
    const machineTs = view.machine_updated_at
      ? `snapshot ${relTimeUtc(view.machine_updated_at)}${machineStale ? ' · 陈旧' : ''}`
      : '';

    return `
      <div class="machine-block${staleClass}">
        <div class="machine-block-head">
          <div class="machine-block-title">${escapeHtml(mid)}</div>
          <div class="machine-block-meta">${escapeHtml(machineTs)}</div>
        </div>
        <dl class="machine-fields">
          <dt>分支</dt><dd>${escapeHtml(p.branch || '(未 checkout)')}</dd>
          <dt>HEAD</dt><dd>${p.head ? escapeHtml(p.head) : '<span class="mute">—</span>'}</dd>
          <dt>upstream</dt><dd>${upstreamLine}</dd>
          <dt>ahead / behind</dt><dd>${aheadBehind}</dd>
          <dt>未提交</dt><dd class="${dirtyCls}">${escapeHtml(dirtyText)}</dd>
          <dt>最近提交</dt><dd class="mute">${escapeHtml(lastCommitRel)}</dd>
          <dt>本机采集</dt><dd class="mute">${escapeHtml(collectedRel)}</dd>
          ${progressLine}
          ${dangerLine}
        </dl>
        ${renderRecentCommits(p)}
      </div>
    `;
  }).join('');

  body.innerHTML = machineBlocks;

  drawer.classList.add('open');
  drawer.setAttribute('aria-hidden', 'false');
  document.body.classList.add('drawer-open');
}

function verdictBannerClass(v) {
  return {
    'synced': 'info',
    'ahead': 'info',
    'one-in-progress': 'warn',
    'diverged': 'err',
    'dual-dirty': 'err',
    'uuid-conflict': 'err',
    'single-machine': 'info',
    'insufficient': 'warn',
  }[v.kind] || 'warn';
}

function verdictBannerIcon(v) {
  if (['diverged', 'dual-dirty', 'uuid-conflict'].includes(v.kind)) return ICON_ALERT_TRIANGLE;
  if (['one-in-progress', 'insufficient'].includes(v.kind)) return ICON_ALERT_CIRCLE;
  // synced / ahead / single-machine → info 态用 check-circle，跟 warn/err 视觉区分（对账 #11）
  return ICON_CHECK_CIRCLE;
}

// -------- 项目管理 --------

// 路径特征启发式识别 os —— machines map 值只有 path 字符串，无 os 元数据。
// P0-H（2026-07-06）：由于 schema v1 未存 machine→os 映射，UI 展示对方机路径时
// 用启发式猜 os；模糊路径落到"Win 路径" 兜底槽位。
function detectOsFromPath(path) {
  if (!path) return null;
  // Windows: 盘符 (C:\ / D:\ / F:\) 或 UNC (\\server\share)
  if (/^[a-zA-Z]:[\\/]/.test(path) || path.startsWith('\\\\')) return 'windows';
  // Mac: /Users/ or ~ 或 $HOME 相对
  if (path.startsWith('/Users/') || path.startsWith('~/') || path.startsWith('$HOME/')) return 'macos';
  if (path.startsWith('/')) return 'macos'; // 兜底 Unix-like
  return null;
}

// 反推 machines map 中 win/mac 各自的 (machine_id, path)。
// 优先用本机 machineId 定位本机；剩余按路径启发式识别对方 os。
//
// P1-G（2026-07-07）：3+ 机项目（如 win + mac + mac-studio）编辑时，
// 原实现只归位 win/mac 两个槽位 → bindProjectForm 提交时只重建 2 entry → 第三台机静默丢失。
// 修复：返回值加 `extras` 数组保留所有未分类的 entries，`bindProjectForm` 提交时先塞 extras 再写 win/mac。
function splitMachinesByOs(machines) {
  const entries = Object.entries(machines || {});
  const result = { winId: '', winPath: '', macId: '', macPath: '', extras: [] };
  for (const [mid, path] of entries) {
    let os = null;
    if (mid === state.machineId) {
      os = state.os;
    } else {
      os = detectOsFromPath(path);
    }
    if (os === 'windows' && !result.winId) {
      result.winId = mid;
      result.winPath = path;
    } else if (os === 'macos' && !result.macId) {
      result.macId = mid;
      result.macPath = path;
    } else {
      // 3+ 机场景 or 无法识别 os：保留到 extras，避免编辑保存时丢失（P1-G 修复）
      result.extras.push({ id: mid, path });
    }
  }
  return result;
}

function renderProjects() {
  const q = document.getElementById('project-query').value.trim().toLowerCase();
  const tbody = document.querySelector('#project-table tbody');
  tbody.innerHTML = '';
  const shared = state.projects.shared.map(p => ({ ...p, _kind: 'shared' }));
  const priv = state.projects.private.map(p => ({ ...p, _kind: 'private' }));
  // v4.2 §16.6：pending 通道删除；只合并 shared + private
  const all = [...shared, ...priv].filter(p => {
    if (!q) return true;
    return (p.name || '').toLowerCase().includes(q) || (p.git_url || '').toLowerCase().includes(q);
  });
  for (const p of all) {
    // machines map 按 machine_id 存真实 hostname，UI 层用启发式猜 os 归位到 Win/Mac 槽位（P0-H 修复）
    const { winPath, macPath } = splitMachinesByOs(p.machines);
    // v4.2 §16.8：删"待 commit project_id" pending badge —— UUID 权威已迁 dashboard-state registry
    const syncBadge = p._kind === 'private'
      ? '<span class="badge mute">仅本机可见</span>'
      : '<span class="badge info">跨机同步</span>';
    const opsDisabled = p._kind === 'private';
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td><strong>${escapeHtml(p.name)}</strong></td>
      <td style="font-family:monospace;font-size:12px">${escapeHtml(p.git_url || '')}</td>
      <td style="color:var(--text-mute)">${escapeHtml(winPath)}</td>
      <td style="color:var(--text-mute)">${escapeHtml(macPath)}</td>
      <td>${syncBadge}</td>
      <td>
        <button class="btn ghost" ${opsDisabled ? 'disabled' : ''} data-action="edit" data-project-id="${p.project_id || ''}">编辑</button>
        <button class="btn danger" data-action="delete" data-project-id="${p.project_id || ''}" data-project-name="${escapeHtml(p.name)}">删除</button>
      </td>
    `;
    tbody.appendChild(tr);
  }
  if (all.length === 0) {
    const message = q && shared.length + priv.length > 0
      ? '无匹配项目，请调整或清空搜索条件'
      : '暂无项目，点击“新增项目”添加';
    tbody.innerHTML = `<tr><td colspan="6" style="text-align:center;color:var(--text-mute);">${message}</td></tr>`;
  }
}

// 编辑 modal 打开时暂存 extras 供 bindProjectForm 提交时合并（P1-G 修复）
let editingExtras = [];
let projectModalPreviousFocus = null;
let projectModalBackground = [];
let projectSaving = false;

function openProjectModal(projectId) {
  projectModalPreviousFocus = document.activeElement;
  state.editingProjectId = projectId;
  editingExtras = [];
  document.getElementById('modal-title').textContent = projectId ? '编辑项目' : '新增项目';
  // Q1-D：打开 modal 重置 Bitbucket web 提示
  const bbHint = document.getElementById('git-bb-hint');
  if (bbHint) bbHint.style.display = 'none';
  document.getElementById('f-sync').disabled = !!projectId;
  document.getElementById('f-sync').title = projectId ? 'sync 状态不可编辑，请删除项目重新新增' : '';
  // 清空 extras 提示条
  const banner = document.getElementById('modal-extras-banner');
  if (banner) banner.remove();
  if (projectId) {
    const p = state.projects.shared.find(x => x.project_id === projectId);
    if (p) {
      document.getElementById('f-name').value = p.name || '';
      document.getElementById('f-git').value = p.git_url || '';
      // 反推 win/mac (machine_id, path) —— machines map 按真实 hostname 存（P0-H 修复）
      // extras 数组保留 3+ 机场景的额外 entries（P1-G 修复）
      const { winId, winPath, macId, macPath, extras } = splitMachinesByOs(p.machines);
      // v4.2 补丁：编辑"对方机已建、本机未加入"的项目时，为本机字段 prefill state.machineId，
      // 让用户直接填路径即可关联本机（原实现只在"新增"分支 prefill，编辑分支撞空 id 校验拦保存）
      let finalWinId = winId, finalMacId = macId;
      if (state.os === 'windows' && !winId) finalWinId = state.machineId;
      if (state.os === 'macos' && !macId) finalMacId = state.machineId;
      document.getElementById('f-win-machine-id').value = finalWinId;
      document.getElementById('f-win-path').value = winPath;
      document.getElementById('f-mac-machine-id').value = finalMacId;
      document.getElementById('f-mac-path').value = macPath;
      document.getElementById('f-sync').checked = true;
      editingExtras = extras || [];
      if (editingExtras.length > 0) {
        // 在 modal 顶部显示提示条：本项目还有 N 台额外机器，保存时会保留
        const b = document.createElement('div');
        b.id = 'modal-extras-banner';
        b.className = 'banner info';
        b.style.marginBottom = '12px';
        const ids = editingExtras.map(e => escapeHtml(e.id)).join(', ');
        b.innerHTML = `ℹ️ 本项目还有 <strong>${editingExtras.length}</strong> 台额外机器: <code>${ids}</code> —— 保存时会保留原样`;
        const form = document.getElementById('project-form');
        form.parentNode.insertBefore(b, form);
      }
    }
  } else {
    document.getElementById('project-form').reset();
    // 本机 machine_id 自动 prefill，对方机字段留空由用户从下拉选或手填（P0-H 修复）
    if (state.os === 'windows') {
      document.getElementById('f-win-machine-id').value = state.machineId;
    } else if (state.os === 'macos') {
      document.getElementById('f-mac-machine-id').value = state.machineId;
    }
  }
  // 每次开弹窗都刷一遍机器列表：别的机器可能刚 pull 进来。
  refreshMachinePickers();
  document.getElementById('project-modal-backdrop').classList.add('open');
  projectModalBackground = [...document.querySelectorAll('.sidebar, .topbar, #main, #drawer')]
    .map(el => ({ el, inert: el.inert }));
  projectModalBackground.forEach(({ el }) => { el.inert = true; });
  document.querySelector('#project-modal-backdrop .modal').scrollTop = 0;
  document.getElementById('f-name').focus({ preventScroll: true });
}

function closeProjectModal(force = false) {
  if (projectSaving && force !== true) return;
  document.getElementById('project-modal-backdrop').classList.remove('open');
  projectModalBackground.forEach(({ el, inert }) => { el.inert = inert; });
  projectModalBackground = [];
  if (projectModalPreviousFocus?.isConnected) projectModalPreviousFocus.focus({ preventScroll: true });
  else document.getElementById('btn-add-project').focus({ preventScroll: true });
}

// 机器下拉：显示主机名（人能认），值存 machine_id（机器匹配用）。
//
// 为什么要有它：身份从主机名改成硬件哈希（`mbp-00000001`）后，用户没法凭记忆手填对方机的
// id 了。但下拉不能取代手填 —— 一台从未同步过的新机器不在列表里，仍需手填兜底，
// 故设计成「选了就填进输入框」，输入框始终是提交的唯一真值来源。
async function refreshMachinePickers() {
  let machines = [];
  try {
    machines = await invoke('list_known_machines', {}) || [];
  } catch (e) {
    console.error('list_known_machines', e);
    return; // 列不出来不影响手填，静默降级
  }
  for (const [pickerId, inputId] of [
    ['f-win-machine-picker', 'f-win-machine-id'],
    ['f-mac-machine-picker', 'f-mac-machine-id'],
  ]) {
    const picker = document.getElementById(pickerId);
    if (!picker) continue;
    const current = picker.value;
    picker.replaceChildren();
    const placeholder = document.createElement('option');
    placeholder.value = '';
    placeholder.textContent = '— 从已知机器选择 —';
    picker.appendChild(placeholder);
    for (const machine of machines) {
      const option = document.createElement('option');
      option.value = machine.machine_id;
      // 主机名可能为空（老 snapshot 没这字段），那就只显示 id，不编造。
      const parts = [machine.hostname || machine.machine_id];
      if (machine.is_local) parts.push('本机');
      if (machine.is_legacy) parts.push('旧式身份·未迁移');
      option.textContent = `${parts.join(' · ')}（${machine.machine_id}）`;
      picker.appendChild(option);
    }
    picker.value = current;
    if (!picker.dataset.bound) {
      picker.addEventListener('change', () => {
        if (!picker.value) return;
        document.getElementById(inputId).value = picker.value;
      });
      picker.dataset.bound = '1';
    }
  }
}

// -------- 本机身份 / 旧身份认领 --------

function renderIdentityFields() {
  const el = document.getElementById('identity-fields');
  if (!el) return;
  const rows = [
    ['machine_id', state.machineId || '（取不到）', state.machineId ? '' : 'danger'],
    ['当次主机名', state.hostname || '（取不到）', state.hostname ? 'mute' : 'warn'],
  ];
  el.replaceChildren();
  for (const [label, value, cls] of rows) {
    const dt = document.createElement('dt');
    dt.textContent = label;
    const dd = document.createElement('dd');
    dd.textContent = value;
    if (cls) dd.className = cls;
    el.append(dt, dd);
  }
}

/// 干跑结果渲染成人能读的清单。**这里显示什么，执行时就改什么** —— 不加工、不省略，
/// 未认领项也照实列出，避免「悄悄跳过」。
function renderIdentityMigrationPlan(plan) {
  const box = document.getElementById('identity-migration-plan');
  if (!box) return;
  if (plan.is_noop || (!plan.registry_edits.length && !plan.snapshot_merge)) {
    box.innerHTML = '<div class="banner info">本机身份已是最新，没有旧身份需要认领。</div>';
    return;
  }
  const section = (title, body) => body
    ? `<div class="form-row"><label>${escapeHtml(title)}</label>${body}</div>`
    : '';
  const claims = plan.claims.map(c =>
    `<li><code>${escapeHtml(c.machine_id)}</code> —— ${escapeHtml(claimReasonText(c.reason))}${
      c.has_snapshot_dir ? '' : '（无历史数据，仅改键名）'}</li>`).join('');
  const edits = plan.registry_edits.map(e =>
    `<li>项目「${escapeHtml(e.project_name)}」：<code>${escapeHtml(e.from_key)}</code> → <code>${escapeHtml(e.to_key)}</code>
     <br><span class="hint">路径不变：${escapeHtml(e.path)}</span></li>`).join('');
  const skipped = plan.skipped.map(s =>
    `<li><code>${escapeHtml(s.machine_id)}</code> —— ${escapeHtml(s.reason)}</li>`).join('');
  const merge = plan.snapshot_merge;
  const mergeHtml = merge
    ? `<ul><li>合并到 <code>${escapeHtml(merge.target)}</code>：共 ${merge.project_count} 个项目、${merge.event_lines} 行事件</li>`
      + merge.source_project_counts.map(([name, count]) =>
        `<li><code>${escapeHtml(name)}</code> 贡献 ${count} 个项目${count === 0 ? '（空壳）' : ''}</li>`).join('')
      + '</ul>'
    : '';
  const conflicts = plan.conflicts.length
    ? `<div class="banner warn">${plan.conflicts.map(escapeHtml).join('<br>')}</div>`
    : '';
  box.innerHTML = conflicts
    + section('将认领这些旧身份', claims ? `<ul>${claims}</ul>` : '')
    + section('注册表键改写（路径值一律不动）', edits ? `<ul>${edits}</ul>` : '')
    + section('快照合并', mergeHtml)
    + section('不动（原样保留）', skipped ? `<ul>${skipped}</ul>` : '')
    + '<div class="banner info">执行前会自动整仓备份；旧目录一个都不删，随时可回滚。</div>';
  document.getElementById('btn-apply-identity-migration').style.display = '';
}

function claimReasonText(reason) {
  switch (reason && reason.kind) {
    case 'frozen_in_local_config': return '本机 local.yaml 记录的旧身份（该文件只有本机写）';
    case 'current_hostname': return '与当次真实主机名一致';
    case 'hostname_history': return '在本机主机名漂移轨迹中出现过';
    default: return '未知理由';
  }
}

function bindIdentityMigration() {
  const planBtn = document.getElementById('btn-plan-identity-migration');
  const applyBtn = document.getElementById('btn-apply-identity-migration');
  if (!planBtn || planBtn.dataset.bound) return;
  planBtn.dataset.bound = '1';
  planBtn.addEventListener('click', async () => {
    try {
      const plan = await invoke('plan_identity_migration', {});
      renderIdentityMigrationPlan(plan);
    } catch (e) {
      flash('检查旧身份失败：' + e, true);
    }
  });
  applyBtn.addEventListener('click', async () => {
    // 必须走 uiConfirm：原生 confirm() 在 Wry macOS 哑弹（详见本文件 uiConfirm 上方注释）
    const ok = await uiConfirm(
      '执行身份迁移？\n\n' +
      '- 执行前会自动整仓备份（备份路径执行完会显示）\n' +
      '- 旧身份目录一个都不删，随时可回滚',
      { title: '执行身份迁移', okText: '执行迁移' }
    );
    if (!ok) return;
    applyBtn.disabled = true;
    try {
      const report = await invoke('apply_identity_migration', {});
      const box = document.getElementById('identity-migration-plan');
      // 未提交 = 共享仓工作树是脏的，后续同步会被 pull-first 拦住（报 [UNKNOWN] 同步失败）。
      // 这种情况必须让用户当场看见，不能只当成功收场。
      const dirtyWarn = report.committed
        ? ''
        : `<div class="banner warn">迁移改动未生成提交（${escapeHtml(report.commit_note || '原因未知')}）。`
          + `共享仓工作树当前是脏的，同步会失败，请先手工处理。</div>`;
      box.innerHTML = `<div class="banner info">迁移完成：改写 ${report.registry_files_written} 条注册表键`
        + `${report.snapshot_written ? '、已合并快照' : ''}。<br>备份位置：<code>${escapeHtml(report.backup_path)}</code>`
        + `<br>保留未删的旧目录：<code>${report.legacy_dirs_kept.map(escapeHtml).join('、') || '无'}</code>`
        + `${report.committed ? '<br>改动已提交到共享仓本地（未推送，下次同步一并推）。' : ''}</div>`
        + dirtyWarn;
      applyBtn.style.display = 'none';
      // 身份已变，重取上下文让全局 state 跟上，否则后续渲染还按旧身份匹配。
      await bootstrapContext();
      // 迁移改写了 registry 的机器键，项目列表必须重取：代码知识页的项目下拉消费的是
      // state.projects（见 repoWikiSelectorEntries），只 bootstrapContext 不刷新它，
      // 会出现「迁移成功但下拉仍只有本机私有项目、非得重启 App 才恢复」。
      await refreshAll();
      renderIdentityFields();
    } catch (e) {
      flash('迁移失败：' + e, true);
    } finally {
      applyBtn.disabled = false;
    }
  });
}

function bindProjectForm() {
  // v4.2 补丁（Mac WKWebView 严格 CSP L3）：取消按钮 inline onclick 挂了，改 addEventListener
  document.getElementById('btn-cancel-project').addEventListener('click', closeProjectModal);
  document.addEventListener('keydown', (event) => {
    const backdrop = document.getElementById('project-modal-backdrop');
    if (!backdrop.classList.contains('open') || document.getElementById('confirm-backdrop').classList.contains('open')) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      closeProjectModal();
    } else if (event.key === 'Tab') {
      const controls = [...backdrop.querySelectorAll('input, select, textarea, button, [href], [tabindex]')]
        .filter(el => !el.disabled && el.tabIndex >= 0 && el.getClientRects().length);
      if (!controls.length) return;
      event.preventDefault();
      const index = controls.indexOf(document.activeElement);
      const next = index < 0 ? (event.shiftKey ? controls.length - 1 : 0)
        : (index + (event.shiftKey ? -1 : 1) + controls.length) % controls.length;
      controls[next].focus();
    }
  });
  // Q1-D：git 地址输入时检测 Bitbucket web 形式（path 含 /projects/.../repos/），提示改填 clone 形式（仅提示不强拦）
  document.getElementById('f-git').addEventListener('input', (ev) => {
    const v = (ev.target.value || '').trim();
    const isBbWeb = /\/projects\/[^/]+\/repos\//i.test(v);
    const bbHint = document.getElementById('git-bb-hint');
    if (bbHint) bbHint.style.display = isBbWeb ? 'block' : 'none';
  });
  document.getElementById('project-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    if (projectSaving) return;
    const name = document.getElementById('f-name').value.trim();
    const git = document.getElementById('f-git').value.trim();
    const winId = document.getElementById('f-win-machine-id').value.trim();
    const winPath = document.getElementById('f-win-path').value.trim();
    const macId = document.getElementById('f-mac-machine-id').value.trim();
    const macPath = document.getElementById('f-mac-path').value.trim();
    const sync = document.getElementById('f-sync').checked;
    if (!name || !git) { flash('项目名 / git 地址必填', true); return; }
    if (winPath && !winId) { flash('填了 Win 路径就必须填 Win 机 machine_id（对方机 hostname）', true); return; }
    if (macPath && !macId) { flash('填了 Mac 路径就必须填 Mac 机 machine_id（对方机 hostname）', true); return; }
    if (winId && macId && winId === macId) { flash('Win 机和 Mac 机 machine_id 不能相同', true); return; }
    const machines = {};
    for (const ex of editingExtras) {
      if (ex.id && ex.path) machines[ex.id] = ex.path;
    }
    if (winId && winPath) machines[winId] = winPath;
    if (macId && macPath) machines[macId] = macPath;
    const localPath = state.os === 'windows' ? winPath : macPath;
    const submitBtn = document.querySelector('#project-form button[type="submit"]');
    const setBtnBusy = (busy, label) => {
      if (!submitBtn) return;
      submitBtn.disabled = busy;
      if (label) submitBtn.textContent = label;
    };
    projectSaving = true;
    document.getElementById('btn-cancel-project').disabled = true;
    setBtnBusy(true, '保存中…');
    try {
      if (state.editingProjectId) {
        // 编辑分支不变（v4.2 edit_project 未改）
        await invoke('edit_project', {
          req: { project_id: state.editingProjectId, name, git_url: git, machines }
        });
        closeProjectModal(true);
        await refreshAll();
        flash('已保存');
        return;
      }

      // v4.2 §16.4 新增分支：
      // sync:true → pull-first 硬约束 → find_project_by_git_url 反查 → 命中弹关联 modal / 未命中直接新建 / 多命中阻塞
      // sync:false → 直接调 add_project（不参与跨机反查）
      if (!sync) {
        await invoke('add_project', {
          req: { name, git_url: git, path: localPath || winPath || macPath, machines, sync: false }
        });
        closeProjectModal(true);
        await refreshAll();
        flash('已新增（仅本机可见）');
        return;
      }

      // ---- sync:true 流程 ----
      setBtnBusy(true, '拉最新 registry…');
      try {
        await invoke('pull_registry_before_write');
      } catch (pullErr) {
        setBtnBusy(false, '保存');
        flash(
          `pull dashboard-state 失败，无法安全新增（v4.2 pull-first 硬约束）：${pullErr}`,
          true
        );
        return;
      }

      setBtnBusy(true, '反查 registry…');
      let lookup;
      try {
        lookup = await invoke('find_project_by_git_url', { gitUrl: git });
      } catch (lookupErr) {
        setBtnBusy(false, '保存');
        flash(`registry 反查失败：${lookupErr}`, true);
        return;
      }
      setBtnBusy(false, '保存');

      if (lookup && lookup.kind === 'conflict') {
        // 多命中 —— URL→UUID 全局唯一 invariant 违反
        const uuids = (lookup.uuids || []).join(', ');
        flash(
          `URL→UUID 冲突：${lookup.url} 命中多个 UUID [${uuids}]，请先去 Fleet Board 删除冗余 UUID 再新增`,
          true
        );
        return; // 阻塞保存
      }

      if (lookup && lookup.kind === 'existing') {
        // 唯一命中 —— 弹关联确认 modal，用户点确认才走关联路径
        const shortId = String(lookup.project_id || '').slice(0, 8);
        const existingMachines = (lookup.machines || []).join(', ');
        const ok = await uiConfirm(
          `该项目已在 registry 注册（UUID ${shortId}…，项目名"${lookup.name}"，已注册机器：${existingMachines}）。\n\n` +
          `是否把本机路径 "${localPath}" 关联到这个 UUID？\n\n` +
          `- 确认：追加 machines[${state.machineId}] = ${localPath}\n` +
          `- 取消：不做任何写入（若你其实要新建独立项目，请确认 git URL 是否填对）`,
          { title: '关联到已有项目', okText: '关联' }
        );
        if (!ok) return;
      }
      // 到这里三种可能：lookup.kind === 'not-found' → 后端 add_project 会生成 UUID 直接落 registry
      //                 lookup.kind === 'existing' + 用户确认 → 后端会走 associate_machine 路径
      // 前端 UI 已经反查过；后端 add_project 会再反查一次做真源（防抢占）——这是有意的双次校验
      // Q1-D：add_project 可能返回 URL 绑定校验的机器可识别错误（tag 前缀），按 tag 分支处理
      const addReq = (confirmDual) => invoke('add_project', {
        req: { name, git_url: git, path: localPath || winPath || macPath, machines, sync: true, confirm_dual_form: confirmDual }
      });
      try {
        await addReq(false);
      } catch (addErr) {
        const msg = String(addErr);
        if (msg.includes('[NEED_DUAL_FORM_CONFIRM]')) {
          // Bitbucket 双形态：本地 origin 与手填是同一仓两种写法，弹确认后带 confirm_dual_form:true 重试
          const ok = await uiConfirm(
            '检测到 Bitbucket 双形态地址：\n\n' +
            '本地 origin 与你填写的 git 地址是同一仓的两种写法（web / clone 形式）。\n\n' +
            '确认后将以本地真实 origin 作主地址、你填写的地址作别名登记（跨机配对靠此对齐）。\n\n是否确认？',
            { title: 'Bitbucket 双形态确认', okText: '确认' }
          );
          if (!ok) { setBtnBusy(false, '保存'); return; }
          setBtnBusy(true, '登记中…');
          await addReq(true);
          setBtnBusy(false, '保存');
        } else if (msg.includes('[ORIGIN_UNRELATED]')) {
          // origin 与手填无关（疑似选错项目目录）：展示陈述句（去机器 tag 前缀），不重试
          setBtnBusy(false, '保存');
          flash(msg.replace(/^.*\[ORIGIN_UNRELATED\]\s*/, ''), true);
          return;
        } else {
          throw addErr; // 其他错误交外层 catch
        }
      }
      closeProjectModal(true);
      await refreshAll();
      flash(lookup && lookup.kind === 'existing' ? '已关联本机路径到已有项目' : '已新增');
    } catch (e) {
      setBtnBusy(false, '保存');
      flash('保存失败：' + e, true);
    } finally {
      projectSaving = false;
      document.getElementById('btn-cancel-project').disabled = false;
      setBtnBusy(false, '保存');
      if (!document.getElementById('project-modal-backdrop').classList.contains('open') &&
          state.activeTab === 'projects' && document.activeElement === document.body) {
        const edit = [...document.querySelectorAll('#project-table [data-action="edit"]')]
          .find(el => el.dataset.projectId === state.editingProjectId);
        (edit || document.getElementById('btn-add-project')).focus({ preventScroll: true });
      }
    }
  });
}

async function editProject(id) {
  if (!id) return;
  openProjectModal(id);
}

// Wry 0.55.1 macOS 未实现原生 confirm() 面板（点击哑弹、立即返回 undefined，删除/关联/双形态确认全失效），
// 统一改走复用 .modal-backdrop 的 DOM 弹窗。Promise 化以顶替同步 confirm：resolve(true)=确认 / false=取消。
// ESC / 遮罩空白 = 取消，Enter 激活当前按钮，Tab 在两个操作间循环；隐藏与项目删除确认默认聚焦取消。
let cancelActiveConfirmation = null;
function uiConfirm(message, { title = '请确认', okText = '确认', danger = false, cancelFirst = false } = {}) {
  // 异步项目操作也可能弹确认；新弹窗先取消旧请求，避免一次确认批准两个动作。
  if (cancelActiveConfirmation) cancelActiveConfirmation();
  return new Promise((resolve) => {
    const previousFocus = document.activeElement;
    const backdrop = document.getElementById('confirm-backdrop');
    const titleEl = document.getElementById('confirm-title');
    const msgEl = document.getElementById('confirm-message');
    const okBtn = document.getElementById('confirm-ok');
    const cancelBtn = document.getElementById('confirm-cancel');
    titleEl.textContent = title;
    msgEl.textContent = message; // textContent 防注入；换行靠 CSS white-space:pre-line 保留
    okBtn.textContent = okText;
    okBtn.classList.toggle('danger', danger);
    okBtn.classList.toggle('primary', !danger);
    let settled = false;
    const close = (val) => {
      if (settled) return; // 幂等：Enter 触发 keydown 与 button click 可能双发，只 resolve 一次
      settled = true;
      cancelActiveConfirmation = null;
      backdrop.classList.remove('open');
      okBtn.removeEventListener('click', onOk);
      cancelBtn.removeEventListener('click', onCancel);
      backdrop.removeEventListener('mousedown', onBackdrop);
      document.removeEventListener('keydown', onKey);
      if (previousFocus?.isConnected) previousFocus.focus();
      resolve(val);
    };
    const onOk = () => close(true);
    const onCancel = () => close(false);
    cancelActiveConfirmation = onCancel;
    const onBackdrop = (e) => { if (e.target === backdrop) close(false); }; // 点遮罩空白 = 取消
    const onKey = (e) => {
      if (e.key === 'Escape') { e.preventDefault(); close(false); }
      else if (e.key === 'Enter') {
        e.preventDefault();
        if (document.activeElement === okBtn) close(true);
        else if (document.activeElement === cancelBtn) close(false);
      } else if (e.key === 'Tab') {
        e.preventDefault();
        (document.activeElement === cancelBtn ? okBtn : cancelBtn).focus();
      }
    };
    okBtn.addEventListener('click', onOk);
    cancelBtn.addEventListener('click', onCancel);
    backdrop.addEventListener('mousedown', onBackdrop);
    document.addEventListener('keydown', onKey);
    backdrop.classList.add('open');
    (cancelFirst ? cancelBtn : okBtn).focus();
  });
}

async function deleteProject(id, name) {
  if (!id) return;
  if (!(await uiConfirm(`删除项目 "${name}"？\n\n仅从注册表移除，项目源码/git 仓/hook 不动。\n若想改 sync 状态请删除后重新新增。`, { title: '删除项目', okText: '删除', danger: true, cancelFirst: true }))) return;
  try {
    await invoke('delete_project', { projectId: id });
    await refreshAll();
    flash('已删除');
  } catch (e) {
    flash('删除失败：' + e, true);
  }
}

// -------- 设置 --------
// 设置页「配置即拉」状态提示：常驻显示上次保存后的同步结果（区别于 3 秒即散的 flash）。
// 用独立 id `settings-sync-status`，不能复用顶栏的 `sync-status`（重复 id 会被 getElementById
// 只取第一个即顶栏，导致设置页状态行永不生效）。
function setSyncStatus(msg, isError = false) {
  const el = document.getElementById('settings-sync-status');
  if (!el) return;
  el.textContent = msg;
  el.style.color = isError ? 'var(--danger, #c0392b)' : 'var(--muted, #888)';
}

async function loadSettings() {
  try {
    const cfg = await invoke('get_settings', {});
    document.getElementById('in-diag').checked = !!cfg.diagnostic_mode;
    document.getElementById('in-state-remote').value = cfg.dashboard_state_remote || '';
    document.getElementById('in-state-root').value = cfg.dashboard_state_root || '';
    document.getElementById('in-repowiki-cli').value = cfg.repowiki_cli_path || '';
    document.getElementById('in-repowiki-data-root').value = cfg.repowiki_data_root || '';
    setSyncStatus('');
  } catch (e) { flash('读设置失败：' + e, true); }

  renderIdentityFields();
  bindIdentityMigration();

  document.getElementById('btn-save-settings').onclick = async () => {
    const diag = document.getElementById('in-diag').checked;
    const remote = document.getElementById('in-state-remote').value.trim();
    const root = document.getElementById('in-state-root').value.trim();
    try {
      // 三态：空串传 null = 显式清空（后端 double_option → Some(None)）；非空 = 设值。
      const resp = await invoke('update_settings', {
        update: {
          diagnostic_mode: diag,
          dashboard_state_remote: remote || null,
          dashboard_state_root: root || null,
        }
      });
      // 命令 Ok 恒表示「保存成功」；同步子状态在 resp.sync（P1-4 部分成功语义）。
      const sync = (resp && resp.sync) || {};
      if (sync.state === 'synced') {
        setSyncStatus('✅ 设置已保存，存储仓已同步');
        flash('设置已保存，存储仓已同步');
      } else if (sync.state === 'unconfigured') {
        setSyncStatus('设置已保存（未配置存储仓地址，未触发同步）');
        flash('设置已保存');
      } else if (sync.state === 'failed') {
        // 前端据 sync.tag 判分支，展示后端给的中文 hint；tag 只用于调试可见。
        setSyncStatus(`⚠️ 设置已保存，但自动同步失败：${sync.hint || ''}（${sync.tag || ''}）`, true);
        flash(`设置已保存，但自动同步失败：${sync.hint || sync.tag || '未知错误'}`, true);
      } else {
        setSyncStatus('设置已保存');
        flash('设置已保存');
      }
    } catch (e) {
      // 走到这里 = 保存本身失败（校验 / 写盘 / 内嵌 token 拒绝等），设置未生效。
      setSyncStatus('❌ 保存失败：' + e, true);
      flash('保存失败：' + e, true);
    }
  };
  document.getElementById('btn-open-data-dir').onclick = async () => {
    try {
      await invoke('open_data_dir', {});
    } catch (e) { flash('打开数据目录失败：' + e, true); }
  };
  document.getElementById('btn-open-session-data-dir').onclick = async () => {
    try { await invoke('open_session_data_dir', {}); }
    catch (e) { flash('打开本机会话目录失败：' + e, true); }
  };
  document.getElementById('btn-save-repowiki-settings').onclick = async () => {
    const repowikiCliPath = document.getElementById('in-repowiki-cli').value.trim();
    const repowikiDataRoot = document.getElementById('in-repowiki-data-root').value.trim();
    try {
      await invoke('update_repowiki_settings', {
        repowikiCliPath: repowikiCliPath || null,
        repowikiDataRoot: repowikiDataRoot || null,
      });
      flash('RepoWiki 本机路径已保存；未触发 dashboard-state 同步');
    } catch (e) { flash('RepoWiki 路径保存失败：' + e, true); }
  };
}

// -------- Utils --------
function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}

function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, c => ({
    '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'
  }[c]));
}

function relTime(mtime) {
  const now = Date.now() / 1000;
  const diff = now - mtime;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff/60)}m前`;
  if (diff < 86400) return `${Math.floor(diff/3600)}h前`;
  return `${Math.floor(diff/86400)}d前`;
}

function absTime(mtime) {
  const d = new Date(mtime * 1000);
  const p = n => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth()+1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// 对话消息 markdown 渲染。后端 preview 每条格式为 "🧑 text" 或 "🤖 text"。
// marked v15 挂 window.marked（对象，v15 起废弃 marked() 直调，只能 marked.parse()）；
// 若加载失败 fallback 到 escapeHtml + <br>（保底不炸）。
function renderDialogMarkdown(previewLines) {
  // marked v15 默认 pass-through raw HTML（如 <observation> <thinking> 等 cc 会话里常见 XML-ish 标签），
  // 塞进 DOM 后浏览器不知道咋渲染非标准标签，会崩布局。pre-escape `<>` 让 marked 完全不当 HTML 处理，
  // 只留 markdown 语法（```/**bold**/list 都不用尖括号），语义无损，且天然免疫 XSS。
  const preEscape = (text) => text.replace(/[<>]/g, c => c === '<' ? '&lt;' : '&gt;');
  const safeParse = (text) => {
    const escaped = preEscape(text);
    if (window.marked && typeof window.marked.parse === 'function') {
      try { return window.marked.parse(escaped); }
      catch (e) { return escapeHtml(text).replace(/\n/g, '<br>'); }
    }
    return escapeHtml(text).replace(/\n/g, '<br>');
  };
  return previewLines.map(line => {
    const m = line.match(/^(🧑|🤖) ?([\s\S]*)$/);
    const role = m ? m[1] : '';
    const text = m ? m[2] : line;
    const roleClass = role === '🧑' ? 'msg-user' : (role === '🤖' ? 'msg-asst' : '');
    const html = safeParse(text);
    return `<div class="msg ${roleClass}"><span class="msg-role">${role}</span><div class="msg-body">${html}</div></div>`;
  }).join('');
}

function flash(msg, isError = false) {
  const el = document.createElement('div');
  el.className = 'banner ' + (isError ? 'err' : 'info');
  el.textContent = msg;
  el.style.position = 'fixed';
  el.style.top = '72px';
  el.style.right = '20px';
  el.style.zIndex = '100';
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 3000);
}

// -------- Mock IPC（浏览器直开 / 审计时用） --------
function mockRepoWikiEnvelope(command, result, warnings = []) {
  return { protocol_version: 1, command, ok: true, result, warnings };
}

// 浏览器直开时的 v2 mock 导航：home + 2 个子页 + 一层孙页（modules 下挂两页），
// 用于验证树形展开/折叠与读页降级；每页配一段 mock 正文（含 page-meta 注释与内链）。
const MOCK_REPOWIKI_NAV = Object.freeze({
  schema_version: 2,
  home_page_id: 'home',
  layout_version: 'mock-1',
  pages: [
    { page_id: 'home', title: '项目总览', path: 'wiki/home.md', page_type: 'home', parent_page_id: null, order: 0, children: ['architecture', 'modules'] },
    { page_id: 'architecture', title: '架构', path: 'wiki/architecture.md', page_type: 'section', parent_page_id: 'home', order: 1, children: [] },
    { page_id: 'modules', title: '模块导航', path: 'wiki/modules/index.md', page_type: 'section', parent_page_id: 'home', order: 2, children: ['module-core', 'module-web'] },
    { page_id: 'module-core', title: '核心模块', path: 'wiki/modules/core.md', page_type: 'module', parent_page_id: 'modules', order: 1, children: [] },
    { page_id: 'module-web', title: 'Web 模块', path: 'wiki/modules/web.md', page_type: 'module', parent_page_id: 'modules', order: 2, children: [] },
  ],
});
const MOCK_REPOWIKI_PAGE_BODY = Object.freeze({
  home: '# 项目总览\n\n浏览器直开时的只读 mock 首页。\n\n[查看架构](repowiki:architecture) · [模块导航](repowiki:modules)',
  architecture: '# 架构\n\n架构页 mock 正文。\n\n![外部图片不会加载](https://example.invalid/image.png)',
  modules: '# 模块导航\n\n模块索引 mock。\n\n[核心模块](repowiki:module-core)',
  'module-core': '# 核心模块\n\n核心模块 mock 正文。',
  'module-web': '# Web 模块\n\nWeb 模块 mock 正文。',
});

// 浏览器直开时的会话列表 mock：每个来源一份，保证四个页签都能看到形态正确的数据
// （mtime 用相对当下的秒数，相对时间列才不会永远显示同一个值）。
const MOCK_SESSIONS_BY_SOURCE = {
  'claude-cli': [
    { session_id: 'mock-sess-01234567', mtime: Date.now()/1000 - 300, cwd: 'F:\\project-dashboard', title: '设计 M2 采集器', projects: 'project-dashboard', haystack: '', source: 'claude-cli' },
    { session_id: 'mock-sess-89abcdef', mtime: Date.now()/1000 - 3600, cwd: 'F:\\project-gamma', title: '', projects: 'project-gamma', haystack: '', source: 'claude-cli' },
  ],
  codex: [
    { session_id: '019c0de0-1234-7abc-8def-1234567890ab', mtime: Date.now()/1000 - 180, cwd: '/Users/mock/Documents/project-dashboard', title: '实现 Codex 会话页签', projects: 'project-dashboard', haystack: '', source: 'codex' },
    { session_id: '019c0de0-aaaa-7bbb-8ccc-ddddeeeeffff', mtime: Date.now()/1000 - 7200, cwd: '/Users/mock/Documents/notes', title: '', projects: 'notes', haystack: '', source: 'codex' },
  ],
  pi: [
    { session_id: '00000000-0000-4000-8000-000000000201', mtime: Date.now()/1000 - 240, cwd: '/Users/mock/Documents/example-tools', title: '给 pi 装工具包', projects: 'example-tools', haystack: '', source: 'pi' },
    { session_id: '00000000-0000-4000-8000-000000000202', mtime: Date.now()/1000 - 5400, cwd: '/Users/mock/Documents', title: '', projects: 'Documents', haystack: '', source: 'pi' },
  ],
  grok: [
    { session_id: '00000000-0000-4000-8000-000000000101', mtime: Date.now()/1000 - 900, cwd: '/Users/mock/Documents/project-dashboard', title: '会话记录加 Grok 来源页签', projects: 'project-dashboard', haystack: '', source: 'grok' },
    { session_id: '00000000-0000-4000-8000-000000000102', mtime: Date.now()/1000 - 86400, cwd: '/Users/mock/Documents', title: '', projects: 'Documents', haystack: '', source: 'grok' },
  ],
};

// resume 命令模板：跟后端 copy_resume_command_for_source 的分支一一对应
const MOCK_RESUME_PREFIX = {
  'claude-cli': 'claude --resume ',
  codex: 'codex resume ',
  pi: 'pi --session ',
  grok: 'grok --resume ',
};

const MOCK_HIDDEN = Object.fromEntries(SESSION_SOURCE_IDS.map(source => [source, new Set()]));

function mockInvoke(cmd, args) {
  if (typeof window !== 'undefined') console.log('[mock]', cmd, args);
  switch (cmd) {
    case 'list_sessions': {
      const source = args?.source || 'claude-cli';
      const visibility = args?.visibility || 'visible';
      if (!SESSION_SOURCE_IDS.includes(source) || !['visible', 'hidden'].includes(visibility)) return Promise.reject(new Error('无效会话查询参数'));
      const query = (args?.query || '').trim().toLowerCase();
      const items = MOCK_SESSIONS_BY_SOURCE[source].filter(item =>
        MOCK_HIDDEN[source].has(item.session_id) === (visibility === 'hidden') &&
        (!query || [item.title, item.cwd, item.projects, item.haystack, item.session_id].some(value => String(value || '').toLowerCase().includes(query)))
      ).sort((a,b) => Number(!!b.favorited) - Number(!!a.favorited) || b.mtime - a.mtime);
      const pageSize = Math.max(1, Math.min(500, args?.pageSize || 20));
      const page = Math.max(1, Math.min(args?.page || 1, Math.max(1, Math.ceil(items.length / pageSize))));
      return Promise.resolve({total:items.length, page, page_size:pageSize, items:items.slice((page-1)*pageSize, page*pageSize)});
    }
    case 'set_session_hidden': {
      if (!MOCK_HIDDEN[args?.source]) return Promise.reject(new Error('不支持的会话来源'));
      if (args.hidden) MOCK_HIDDEN[args.source].add(args.sessionId);
      else MOCK_HIDDEN[args.source].delete(args.sessionId);
      return Promise.resolve({source:args.source, session_id:args.sessionId, hidden:args.hidden});
    }
    case 'toggle_favorite': {
      const items = Object.values(MOCK_SESSIONS_BY_SOURCE).flat().filter(s => s.session_id === args.sessionId);
      const next = !items[0]?.favorited;
      items.forEach(s => { s.favorited = next; });
      return Promise.resolve(next);
    }
    case 'list_projects':
      // v4.2 §16.6：pending 通道删除
      return Promise.resolve({
        shared: [],
        private: [{ name: 'company-x', git_url: 'https://gitlab.example.com/team/company-x', path: 'F:\\company-x', sync: false }]
      });
    case 'aggregate_fleet_board':
      return Promise.resolve([]);
    case 'pull_registry_before_write':
      return Promise.resolve(null); // mock 环境假装 pull 成功
    case 'find_project_by_git_url':
      return Promise.resolve({ kind: 'not-found', git_url_normalized: (args?.gitUrl || '').toLowerCase() });
    case 'get_settings':
      return Promise.resolve({
        schema_version: 1,
        machine_id: 'MOCK',
        sync_tick_minutes: 3,
        diagnostic_mode: false,
        dashboard_state_remote: '',
        dashboard_state_root: '',
        repowiki_cli_path: '',
        repowiki_data_root: '',
      });
    case 'update_settings': {
      // 新回传体：{ config, sync }。mock 无存储仓 → sync=unconfigured（未触发同步）。
      const upd = args?.update || {};
      const remote = upd.dashboard_state_remote;
      return Promise.resolve({
        config: { schema_version: 1, machine_id: 'MOCK', ...upd },
        sync: { state: remote ? 'synced' : 'unconfigured' },
      });
    }
    case 'open_data_dir':
    case 'open_session_data_dir':
      return Promise.resolve(null); // mock 环境假装打开成功
    case 'update_repowiki_settings':
      return Promise.resolve({
        schema_version: 1,
        machine_id: 'MOCK',
        repowiki_cli_path: args?.repowikiCliPath || null,
        repowiki_data_root: args?.repowikiDataRoot || null,
      });
    case 'repowiki_status':
      return Promise.resolve(mockRepoWikiEnvelope('status', {
        initialization: 'initialized',
        repo_id: 'rwrepo_mock',
        repository: { branch_ref: 'refs/heads/main', head_commit: '1'.repeat(40) },
        wiki: { freshness: 'current', integrity: 'healthy', generation_id: 'rwwiki_mock_generation' },
        graph: { availability: 'ready', integrity: 'healthy', source_freshness: 'current', generation_id: 'rwgen_mock_generation' },
        review: {
          integrity: 'healthy',
          // v2 无固定页集：只给少量按动态 page_id 键入的 mock 审查态，其余页面渲染为未知（'—'）。
          pages: { architecture: 'reviewed', 'module-core': 'stale' },
          totals: { total: 5, reviewed: 1, unreviewed: 3, stale: 1 },
        },
      }));
    case 'repowiki_read_project':
      return Promise.resolve(mockRepoWikiEnvelope('read project', {
        repo_id: 'rwrepo_mock',
        repository: { branch_ref: 'refs/heads/main', head_commit: '1'.repeat(40) },
        wiki: { generation_id: 'rwwiki_mock_generation', source_commit: '1'.repeat(40), graph_generation_id: 'rwgen_mock_generation' },
        navigation: MOCK_REPOWIKI_NAV,
      }));
    case 'repowiki_read_page': {
      const page = MOCK_REPOWIKI_NAV.pages.find(p => p.page_id === args?.pageId)
        || MOCK_REPOWIKI_NAV.pages.find(p => p.page_id === MOCK_REPOWIKI_NAV.home_page_id);
      const body = MOCK_REPOWIKI_PAGE_BODY[page.page_id] || `# ${page.title}\n\n（无 mock 正文）`;
      // 正文开头带 page-meta 注释行，验证前端剥离逻辑（渲染时不应出现该注释）。
      const content = `<!-- repowiki:page-meta {"page_id":"${page.page_id}","generation":"rwwiki_mock_generation"} -->\n${body}`;
      return Promise.resolve(mockRepoWikiEnvelope('read page', {
        page_id: page.page_id,
        title: page.title,
        path: page.path,
        content,
        content_sha256: '2'.repeat(64),
        evidence_fingerprint: '3'.repeat(64),
        wiki_generation_id: 'rwwiki_mock_generation',
      }));
    }
    case 'repowiki_read_generations':
      // v2 直读代次形状（后端 read_generations）：wiki 段带 status/head_commit/page_total，
      // review 段只给 totals；graph 仍由 status 提供，故这里不再伪造 graph。
      return Promise.resolve(mockRepoWikiEnvelope('read generations', {
        branch: { branch_ref: 'refs/heads/main' },
        wiki: {
          generation_id: 'rwwiki_mock_generation',
          status: 'complete',
          integrity: 'healthy',
          layout_version: 3,
          mode: 'update',
          generated_at: '2020-01-02T00:00:00+00:00',
          head_commit: '1'.repeat(40),
          graph_generation_id: 'rwgen_mock_generation',
          page_total: MOCK_REPOWIKI_NAV.pages.length,
        },
        review: {
          totals: { total: MOCK_REPOWIKI_NAV.pages.length, reviewed: 0, unreviewed: MOCK_REPOWIKI_NAV.pages.length },
        },
      }));
    case 'repowiki_graph_search': {
      const node = {
        node_id: 'a'.repeat(64),
        qualified_name: `${args?.text || 'process'}::handler`,
        name: 'handler',
        kind: 'function',
        path: 'src/service.py',
        line: 12,
      };
      return Promise.resolve(mockRepoWikiEnvelope('graph search', { query: args?.text || '', nodes: [node] }));
    }
    case 'repowiki_graph_show':
      return Promise.resolve(mockRepoWikiEnvelope('graph show', {
        node: { node_id: args?.nodeId || 'a'.repeat(64), qualified_name: 'service::handler', name: 'handler', kind: 'function', path: 'src/service.py', line: 12 },
        edges: [{ source_id: 'b'.repeat(64), target_id: args?.nodeId || 'a'.repeat(64), type: 'CALLS' }],
        truncated: false,
      }));
    case 'repowiki_graph_callers':
    case 'repowiki_graph_callees':
    case 'repowiki_graph_impact': {
      const operation = cmd.slice('repowiki_graph_'.length);
      return Promise.resolve(mockRepoWikiEnvelope(`graph ${operation}`, {
        node_id: args?.nodeId || 'a'.repeat(64),
        depth: args?.depth || 2,
        edge_types: ['CALLS'],
        nodes: [{ node_id: 'b'.repeat(64), qualified_name: `service::${operation}`, name: operation, kind: 'function', path: 'src/flow.py', line: 24, hop: 1 }],
        edges: [{ source_id: 'b'.repeat(64), target_id: args?.nodeId || 'a'.repeat(64), type: 'CALLS' }],
        truncated: false,
      }));
    }
    case 'get_sync_status':
      return Promise.resolve({ last_success_at: null, last_attempt: null });
    case 'session_detail':
      return Promise.resolve({
        preview: ['🧑 mock question', '🤖 mock answer'],
        // grok 的生产逻辑「改过的文件」恒为空（chat_history 里没有工具入参），
        // mock 必须跟着空，否则浏览器直开会看到一个真实环境永远不会出现的文件
        files: args?.source === 'grok' ? [] : ['F:\\mock\\file.rs'],
      });
    case 'copy_resume_command':
      return Promise.resolve(
        (MOCK_RESUME_PREFIX[args?.source] || MOCK_RESUME_PREFIX['claude-cli'])
        + (args?.sessionId || 'mock'),
      );
    case 'get_local_context':
      // 形状对齐后端 MachineContext：machine_id 可为 null（取不到时前端必须报错而非兜底）。
      return Promise.resolve({
        machine_id: 'mock-1a2b3c4d',
        machine_id_error: null,
        hostname: 'MOCK-HOST',
        os: 'windows',
      });
    case 'list_known_machines':
      return Promise.resolve([
        { machine_id: 'mock-1a2b3c4d', hostname: 'MOCK-HOST', is_local: true, is_legacy: false },
        { machine_id: 'DESKTOP-DEMO01', hostname: '', is_local: false, is_legacy: true },
      ]);
    case 'plan_identity_migration':
      return Promise.resolve({
        target_machine_id: 'mock-1a2b3c4d',
        current_hostname: 'MOCK-HOST',
        current_frozen_id: 'MOCK-HOST-OLD',
        claims: [{ machine_id: 'MOCK-HOST-OLD', reason: { kind: 'frozen_in_local_config' }, has_snapshot_dir: true }],
        skipped: [{ machine_id: 'DESKTOP-DEMO01', reason: '无证据表明属于本机——原样保留' }],
        registry_edits: [],
        snapshot_merge: null,
        conflicts: [],
        is_noop: false,
      });
    default:
      return Promise.resolve(null);
  }
}

if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    SESSION_SOURCE_IDS,
    sessionSourceLabel,
    createSessionViews,
    mockInvoke,
    renderSafeWikiMarkdown,
    repoWikiStatusNotice,
    briefVersion,
    repoWikiSelectorEntries,
    unwrapRepoWikiEnvelope,
    computeSyncStatusView,
    validatedRepoWikiNavigation,
    stripRepoWikiPageMeta,
    repoWikiPageIdFromLink,
    repoWikiV2Freshness,
    wikiStateLabel,
  };
}
