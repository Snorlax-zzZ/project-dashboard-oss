const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const distDir = path.join(__dirname, '..', 'dist');
const html = fs.readFileSync(path.join(distDir, 'index.html'), 'utf8');
const appSource = fs.readFileSync(path.join(distDir, 'app.js'), 'utf8');
const ui = require(path.join(distDir, 'app.js'));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function createClassList() {
  const values = new Set();
  return {
    add: (...names) => names.forEach(name => values.add(name)),
    remove: (...names) => names.forEach(name => values.delete(name)),
    toggle: (name, force) => {
      if (force === true) values.add(name);
      else if (force === false) values.delete(name);
      else if (values.has(name)) values.delete(name);
      else values.add(name);
    },
    contains: name => values.has(name),
  };
}

function createElement(id = '') {
  const attributes = new Map();
  return {
    id,
    value: '',
    innerHTML: '',
    textContent: '',
    dataset: {},
    style: {},
    classList: createClassList(),
    setAttribute: (name, value) => attributes.set(name, String(value)),
    getAttribute: name => attributes.get(name) ?? null,
    addEventListener: () => {},
    querySelectorAll: () => [],
    appendChild: () => {},
    closest: () => null,
  };
}

function runtimeSource() {
  const mutation = process.env.SESSION_TABS_TEST_MUTATION;
  if (!mutation) return appSource;

  const guards = {
    'drop-list-success-guard':
      'if (requestSerial !== state.sessionListRequestSerial[source]) return;',
    'drop-detail-success-guard':
      'if (requestSerial !== state.sessionDetailRequestSerial || detailKey !== state.sessionDetailKey) return;',
  };
  const guard = guards[mutation];
  assert.ok(guard, `未知测试变异：${mutation}`);
  const mutated = appSource.replace(guard, '');
  assert.notEqual(mutated, appSource, `测试变异没有命中生产守卫：${mutation}`);
  return mutated;
}

function createRuntimeHarness(invokeImpl) {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, createElement(id));
    return elements.get(id);
  };
  const document = {
    body: createElement('body'),
    addEventListener: () => {},
    getElementById: element,
    querySelector: () => null,
    querySelectorAll: () => [],
    createElement,
  };
  const context = {
    console,
    document,
    module: { exports: {} },
    exports: {},
    require,
    setTimeout,
    clearTimeout,
    window: {
      __TAURI__: { core: { invoke: invokeImpl } },
    },
  };
  context.globalThis = context;
  vm.createContext(context);
  vm.runInContext(runtimeSource(), context, { filename: 'dashboard-tauri/dist/app.js' });
  return {
    context,
    element,
    run: source => vm.runInContext(source, context),
  };
}

function testSourceTabsMarkup() {
  assert.match(
    html,
    /class="session-source-tabs"[^>]*role="tablist"/,
    'Session Board 需要独立的来源 tablist',
  );
  assert.match(
    html,
    /data-session-source="claude-cli"[^>]*aria-selected="true"[^>]*>[\s\S]*?Claude Code/,
    'Claude Code 必须是默认来源',
  );
  assert.match(
    html,
    /data-session-source="codex"[^>]*aria-selected="false"[^>]*>[\s\S]*?Codex/,
    '必须存在 Codex 来源页签',
  );
  assert.match(
    html,
    /data-session-source="pi"[^>]*aria-selected="false"[^>]*>[\s\S]*?pi/,
    '必须存在 pi 来源页签',
  );
  assert.match(
    html,
    /data-session-source="grok"[^>]*aria-selected="false"[^>]*>[\s\S]*?Grok/,
    '必须存在 Grok 来源页签',
  );
  // 四个页签都要有完整的 tab 语义属性，键盘导航与读屏才不会漏项
  const tabs = [...html.matchAll(/data-session-source="([^"]+)"/g)].map(match => match[1]);
  assert.deepEqual(tabs, ['claude-cli', 'codex', 'pi', 'grok', 'opencode', 'antigravity', 'dsh', 'zcode', 'gemini-cli'], '页签顺序固定为九来源');
  for (const source of tabs) {
    const markup = html.match(new RegExp(`<button[^>]*data-session-source="${source}"[^>]*>`))[0];
    assert.match(markup, /role="tab"/, `${source} 页签缺 role=tab`);
    assert.match(markup, /aria-controls="session-source-panel"/, `${source} 页签缺 aria-controls`);
    assert.match(markup, /tabindex="(0|-1)"/, `${source} 页签缺 tabindex`);
  }
  assert.equal(
    (html.match(/aria-selected="true"/g) || []).length,
    1,
    '同一时刻只能有一个页签是选中态',
  );
}

function testSourceIdsAndLabels() {
  assert.deepEqual(
    [...ui.SESSION_SOURCE_IDS],
    ['claude-cli', 'codex', 'pi', 'grok', 'opencode', 'antigravity', 'dsh', 'zcode', 'gemini-cli'],
    '来源 id 列表必须跟后端 allowlist 一致',
  );
  assert.equal(ui.sessionSourceLabel('claude-cli'), 'Claude Code');
  assert.equal(ui.sessionSourceLabel('codex'), 'Codex');
  assert.equal(ui.sessionSourceLabel('pi'), 'pi');
  assert.equal(ui.sessionSourceLabel('grok'), 'Grok');
  for (const [id,label] of Object.entries({opencode:'OpenCode', antigravity:'Antigravity CLI', dsh:'DSH', zcode:'ZCode', 'gemini-cli':'Gemini CLI'})) assert.equal(ui.sessionSourceLabel(id),label);
  assert.equal(
    ui.sessionSourceLabel('unknown-source'),
    'unknown-source',
    '未知来源回落到 id 本身，不能显示成 Claude Code',
  );
}

function testIndependentViewStateFactory() {
  const views = ui.createSessionViews();
  assert.deepEqual(Object.keys(views), ['claude-cli', 'codex', 'pi', 'grok', 'opencode', 'antigravity', 'dsh', 'zcode', 'gemini-cli']);
  const objects = new Set(Object.values(views));
  assert.equal(objects.size, 9, '九个来源不能共享状态对象');
  views['claude-cli'].query = 'alpha';
  views['claude-cli'].page = 3;
  for (const source of ['codex', 'pi', 'grok']) {
    assert.equal(views[source].query, '', `${source} 不该被 claude-cli 的搜索词污染`);
    assert.equal(views[source].page, 1);
    assert.equal(views[source].pageSize, 20);
    assert.equal(views[source].loading, false);
  }
}

async function testMockListAndResumeAreSourceAware() {
  const expectedResume = {
    'claude-cli': 'claude --resume ',
    codex: 'codex resume ',
    pi: 'pi --session ',
    grok: 'grok --resume ',
    opencode: 'opencode --session ',
    antigravity: 'agy --conversation ',
  };
  const seen = [];
  for (const source of ui.SESSION_SOURCE_IDS) {
    const page = await ui.mockInvoke('list_sessions', { source, page: 1, pageSize: 20 });
    assert.ok(page.items.length > 0, `${source} mock 列表不能为空`);
    assert.ok(
      page.items.every(item => item.source === source),
      `${source} mock 数据的 source 字段必须自洽`,
    );
    seen.push(JSON.stringify(page.items));
    if (expectedResume[source]) {
      assert.equal(await ui.mockInvoke('copy_resume_command', { source, sessionId: `${source}-sid` }), `${expectedResume[source]}${source}-sid`);
    } else {
      await assert.rejects(ui.mockInvoke('copy_resume_command', { source, sessionId: `${source}-sid` }), /RESUME_UNSUPPORTED/);
    }
  }
  assert.equal(new Set(seen).size, ui.SESSION_SOURCE_IDS.length, '各来源 mock 数据必须互不相同');

  const summary = await ui.mockInvoke('session_detail', {source:'antigravity', sessionId:'sample'});
  assert.deepEqual(summary.preview, []);
  assert.ok(summary.summary);
  await assert.rejects(ui.mockInvoke('copy_resume_command', {source:'unknown', sessionId:'sample'}));
  // mock 的详情必须跟生产语义一致：grok 的「改过的文件」在生产逻辑里恒为空
  const grokDetail = await ui.mockInvoke('session_detail', { source: 'grok', sessionId: 'x' });
  assert.deepEqual(grokDetail.files, [], 'grok mock 不得伪造改动文件');
  assert.ok(grokDetail.preview.length > 0, 'grok mock 仍应有对话预览');
  const piDetail = await ui.mockInvoke('session_detail', { source: 'pi', sessionId: 'x' });
  assert.ok(piDetail.files.length > 0, 'pi 生产逻辑能拿到改动文件，mock 不该是空的');
}

function testProductionIpcCarriesSource() {
  assert.match(
    appSource,
    /invoke\('list_sessions',\s*\{\s*source,/,
    'list_sessions IPC 必须显式传 source',
  );
  assert.match(
    appSource,
    /invoke\('session_detail',\s*\{\s*source,/,
    'session_detail IPC 必须显式传 source',
  );
  assert.match(
    appSource,
    /invoke\('copy_resume_command',\s*\{\s*source,/,
    'copy_resume_command IPC 必须显式传 source',
  );
  assert.match(appSource, /读取中…/, '来源首次加载不得短暂误报“暂无活动会话”');
}

async function testStaleListResponseCannotOverwriteLatestResult() {
  const first = deferred();
  const second = deferred();
  const requests = [];
  const harness = createRuntimeHarness((cmd, args) => {
    assert.equal(cmd, 'list_sessions');
    requests.push(args);
    return requests.length === 1 ? first.promise : second.promise;
  });
  harness.run(`
    state.sessionViews['claude-cli'].items = [{ session_id: 'baseline' }];
    document.getElementById('session-query').value = 'first';
  `);
  const firstRequest = harness.run('fetchSessions(1, 20)');
  harness.element('session-query').value = 'second';
  const secondRequest = harness.run('fetchSessions(1, 20)');
  harness.run("state.sessionSource = 'codex'");

  second.resolve({
    items: [{ session_id: 'latest' }],
    page: 1,
    page_size: 20,
    total: 1,
  });
  await secondRequest;
  first.resolve({
    items: [{ session_id: 'stale' }],
    page: 1,
    page_size: 20,
    total: 1,
  });
  await firstRequest;

  assert.deepEqual(
    JSON.parse(harness.run("JSON.stringify(state.sessionViews['claude-cli'].items)")),
    [{ session_id: 'latest' }],
    '同一来源较早发出的列表响应晚到时，不得覆盖较新的结果',
  );
  assert.deepEqual(
    requests.map(request => request.query),
    ['first', 'second'],
    '测试必须真实发出两个不同查询，不能只断言模拟数据',
  );
}

async function testStaleDetailResponseCannotOverwriteCurrentDrawer() {
  const first = deferred();
  const second = deferred();
  let calls = 0;
  const harness = createRuntimeHarness((cmd) => {
    assert.equal(cmd, 'session_detail');
    calls += 1;
    return calls === 1 ? first.promise : second.promise;
  });

  const firstRequest = harness.run(`
    showSessionDetail({
      source: 'claude-cli',
      session_id: 'first-session',
      title: 'first'
    })
  `);
  const secondRequest = harness.run(`
    showSessionDetail({
      source: 'codex',
      session_id: 'second-session',
      title: 'second'
    })
  `);
  second.resolve({ preview: ['🤖 最新详情'], files: ['latest.rs'] });
  await secondRequest;
  first.resolve({ preview: ['🤖 过期详情'], files: ['stale.rs'] });
  await firstRequest;

  assert.match(harness.element('drawer-body').innerHTML, /最新详情/);
  assert.match(harness.element('drawer-body').innerHTML, /latest\.rs/);
  assert.doesNotMatch(
    harness.element('drawer-body').innerHTML,
    /过期详情|stale\.rs/,
    '旧来源详情晚到时不得覆盖当前抽屉',
  );
}

async function testEmptyDetailExplainsBoundedPreviewWithoutHidingFiles() {
  let detail = { preview: [], files: ['still-visible.rs'] };
  const harness = createRuntimeHarness(async cmd => {
    assert.equal(cmd, 'session_detail');
    if (detail instanceof Error) throw detail;
    return detail;
  });
  const show = () => harness.run("showSessionDetail({source:'codex', session_id:'empty-session', title:'test'})");
  await show();
  assert.match(harness.element('drawer-body').innerHTML, /最近 512KB 中未找到可显示的对话/);
  assert.match(harness.element('drawer-body').innerHTML, /复制 resume 命令/);
  assert.match(harness.element('drawer-body').innerHTML, /still-visible\.rs/);

  detail = { preview: ['🧑 正常对话 <tag>'], files: [] };
  await show();
  assert.match(harness.element('drawer-body').innerHTML, /正常对话 &lt;tag&gt;/);
  assert.doesNotMatch(harness.element('drawer-body').innerHTML, /未找到可显示的对话/);
  detail = new Error('read failed');
  await show();
  assert.match(harness.element('drawer-body').innerHTML, /加载详情失败: Error: read failed/);
}

async function testPartialCapabilitiesAreVisible() {
  const harness = createRuntimeHarness(async () => ({preview:[], files:[], summary:'仅摘要 <script>', notice:'暂不支持完整对话'}));
  await harness.run(`showSessionDetail({source:'antigravity',session_id:'test-id',title:'合成摘要',mtime:1})`);
  assert.match(harness.element('session-inspector').innerHTML, /仅摘要 &lt;script&gt;/);
  assert.match(harness.element('session-inspector').innerHTML, /暂不支持完整对话/);
  assert.doesNotMatch(harness.element('session-inspector').innerHTML, /最近对话/);
  await harness.run(`showSessionDetail({source:'zcode',session_id:'test-id',title:'合成会话',mtime:1})`);
  assert.match(harness.element('session-inspector').innerHTML, /复制会话 ID/);
  assert.doesNotMatch(harness.element('session-inspector').innerHTML, /claude --resume|复制恢复命令/);
}

async function main() {
  await testPartialCapabilitiesAreVisible();
  testSourceTabsMarkup();
  testSourceIdsAndLabels();
  testIndependentViewStateFactory();
  await testMockListAndResumeAreSourceAware();
  testProductionIpcCarriesSource();
  await testStaleListResponseCannotOverwriteLatestResult();
  await testStaleDetailResponseCannotOverwriteCurrentDrawer();
  await testEmptyDetailExplainsBoundedPreviewWithoutHidingFiles();
  console.log('session-source-tabs.test.js: ok');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
