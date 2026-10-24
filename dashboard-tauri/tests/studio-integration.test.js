const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const dist = path.join(__dirname, '../dist');
const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
const app = fs.readFileSync(path.join(dist, 'app.js'), 'utf8');
function element(id) {
  const classes = new Set();
  return { id, innerHTML: '', textContent: '', value: '', dataset: {}, style: {}, checked: false,
    classList: { add: (...xs) => xs.forEach(x => classes.add(x)), remove: (...xs) => xs.forEach(x => classes.delete(x)),
      contains: x => classes.has(x), toggle: (x, on) => (on ?? !classes.has(x)) ? classes.add(x) : classes.delete(x) },
    setAttribute() {}, addEventListener() {}, querySelectorAll: () => [], querySelector: () => null,
    contains: () => false, appendChild() {}, append() {}, replaceChildren() {}, focus() {} };
}
function runtime(handler = async () => ({ preview: [], files: [] })) {
  // 只提供真实 HTML 中存在的节点，避免桩自动创建不存在的设置控件。
  const nodes = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => [id, element(id)]));
  const calls = [];
  const context = { console, module: { exports: {} }, setTimeout: () => 0, clearTimeout() {},
    navigator: { clipboard: { writeText: async () => {} } },
    document: { body: element('body'), addEventListener() {}, getElementById: id => nodes.get(id) || null,
      querySelectorAll: () => [], querySelector: () => null, createElement: () => element('') },
    window: { __TAURI__: { core: { invoke: async (cmd, args) => { calls.push({ cmd, args }); return handler(cmd, args); } } } } };
  vm.createContext(context);
  vm.runInContext(app, context);
  return { nodes, context, calls, run: code => vm.runInContext(code, context) };
}
const session = { source: 'claude-cli', session_id: '11111111-1111-4111-8111-111111111111', title: '会话甲', mtime: 1 };

test('侧栏显示后端真实身份，身份失败时不残留原型机器 ID', async () => {
  const rt = runtime(async () => ({ machine_id: 'desktop-test123', hostname: 'test-host', os: 'windows' }));
  await rt.run('bootstrapContext()');
  assert.equal(rt.nodes.get('sidebar-machine-id')?.textContent, 'desktop-test123');
  const failed = runtime(async () => ({ machine_id: null, machine_id_error: 'unavailable' }));
  await failed.run('bootstrapContext()');
  assert.equal(failed.nodes.get('sidebar-machine-id')?.textContent, '身份不可用');
});

test('新设置页读取与两类保存沿用真实控件和原 IPC 契约', async () => {
  const cfg = { diagnostic_mode: true, dashboard_state_remote: 'git@example/repo', dashboard_state_root: 'F:/state',
    repowiki_cli_path: 'E:/tools/repowiki/repowiki.exe', repowiki_data_root: 'E:/data/wiki' };
  const rt = runtime(async cmd => cmd === 'get_settings' ? cfg : { sync: { state: 'unconfigured' } });
  await rt.run('loadSettings()');
  assert.equal(rt.nodes.get('in-state-root').value, 'F:/state');
  assert.equal(rt.nodes.get('in-repowiki-cli').value, cfg.repowiki_cli_path);
  await rt.nodes.get('btn-save-settings').onclick();
  await rt.nodes.get('btn-save-repowiki-settings').onclick();
  const updates = rt.calls.filter(x => x.cmd === 'update_settings').map(x => JSON.parse(JSON.stringify(x.args.update)));
  assert.deepEqual(updates, [{ diagnostic_mode: true, dashboard_state_remote: cfg.dashboard_state_remote, dashboard_state_root: cfg.dashboard_state_root }]);
  assert.deepEqual(JSON.parse(JSON.stringify(rt.calls.find(x => x.cmd === 'update_repowiki_settings').args)),
    { repowikiCliPath: cfg.repowiki_cli_path, repowikiDataRoot: cfg.repowiki_data_root });
  assert.ok(rt.nodes.get('btn-open-data-dir').onclick);
  assert.ok(rt.nodes.get('btn-open-session-data-dir').onclick);
  assert.ok(!rt.calls.some(x => /migration|sync_now/.test(x.cmd)));
});

test('双栏会话详情只显示 Inspector，不同时弹出旧抽屉', async () => {
  const rt = runtime(); rt.context.sample = session;
  await rt.run('showSessionDetail(sample)');
  assert.match(rt.nodes.get('session-inspector').innerHTML, /会话甲/);
  assert.equal(rt.nodes.get('drawer').classList.contains('open'), false);
  assert.equal(rt.context.document.body.classList.contains('drawer-open'), false);
});

test('关闭或切换来源后清空 Inspector，晚到请求不能恢复旧会话', async () => {
  let done; const pending = new Promise(resolve => { done = resolve; });
  const rt = runtime(() => pending); rt.context.sample = session;
  const loading = rt.run('showSessionDetail(sample)');
  rt.run('closeDrawer()');
  done({ preview: [], files: [] }); await loading;
  assert.doesNotMatch(rt.nodes.get('session-inspector').innerHTML, /会话甲/);
  assert.match(rt.nodes.get('session-inspector').innerHTML, /未选择会话/);
});

test('Inspector 展示的四源恢复命令与后端契约一致', () => {
  const rt = runtime(); rt.context.sid = session.session_id;
  for (const [source, command] of [['claude-cli','claude --resume'],['codex','codex resume'],['pi','pi --session'],['grok','grok --resume']]) {
    rt.context.source = source;
    assert.equal(rt.run('getResumeCmdPreview(source, sid)'), `${command} ${session.session_id}`);
  }
});

test('复制失败返回明确失败，不能显示已复制', async () => {
  const rt = runtime(async () => { throw new Error('clipboard denied'); }); rt.context.sid = session.session_id;
  assert.equal(await rt.run('copyResumeCmd("codex", sid)'), false);
});

test('Fleet 双栏保留完整快照字段，不丢失 upstream、进度和危险状态', () => {
  const rt = runtime();
  rt.context.aggregate = { name: '项目甲', verdict: { kind: 'single-machine', machine_id: 'win' },
    machines: { win: { project: { branch: 'main', head: 'abc12345', upstream: 'origin/main', ahead: 2, behind: 0,
      dirty: 0, untracked: 0, progress: { done: 1, total: 3, next: '校验发布' }, dangerous_state: 'rebase' } } } };
  rt.run('showAggregateDetail(aggregate)');
  const inspector = rt.nodes.get('fleet-inspector').innerHTML;
  for (const field of ['origin/main', '校验发布', 'rebase']) assert.ok(inspector.includes(field), field);
  assert.equal(rt.nodes.get('drawer').classList.contains('open'), false);
});

test('切换浅色再切回深色保留选中的深色色盘', () => {
  const values = new Map([['project-dashboard.appearance','dark'],['project-dashboard.palette','nord']]);
  let toggle;
  const context = { window: { matchMedia: () => ({ matches: true }), localStorage: {
    getItem: key => values.get(key), setItem: (key, value) => values.set(key, value) } },
    document: { readyState: 'complete', documentElement: { dataset: {} }, addEventListener() {}, querySelectorAll: () => [],
      getElementById: id => id === 'btn-theme-toggle' ? { setAttribute() {}, addEventListener: (_, fn) => { toggle = fn; } } : null } };
  vm.runInNewContext(fs.readFileSync(path.join(dist, 'appearance.js'), 'utf8'), context);
  toggle(); toggle();
  assert.equal(context.document.documentElement.dataset.palette, 'nord');
});
