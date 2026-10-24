const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const appPath = path.join(__dirname, '..', 'dist', 'app.js');
const ui = require(appPath);

async function testSafeMarkdown() {
  // 内链白名单以 state.repowiki.pageIds 为准（v2 动态目录），先载入一个含 architecture 的导航。
  ui.validatedRepoWikiNavigation({
    navigation: {
      schema_version: 2,
      home_page_id: 'home',
      pages: [
        { page_id: 'home', title: '首页', path: 'wiki/home.md', parent_page_id: null, order: 0 },
        { page_id: 'architecture', title: '架构', path: 'wiki/architecture.md', parent_page_id: 'home', order: 1 },
      ],
    },
  });
  const dangerous = [
    '# Overview',
    '<b>raw html remains text</b>',
    '<script>window.__pwned = true</script>',
    '<style>body{display:none}</style>',
    '<iframe src="https://evil.example/frame"></iframe>',
    '<object data="https://evil.example/object"></object>',
    '<img src="https://evil.example/raw.png" onerror="alert(1)">',
    '<svg onload="alert(1)"><image href="https://evil.example/vector.png"></image></svg>',
    '<div onerror="alert(1)">ordinary raw html</div>',
    '![remote](https://evil.example/markdown.png)',
    '![data image](data:image/svg+xml,<svg onload=alert(1)></svg>)',
    '[external](https://evil.example/page)',
    '[script](javascript:alert(1))',
    '[架构](repowiki:architecture)',
  ].join('\n\n');
  const html = ui.renderSafeWikiMarkdown(dangerous);
  assert.match(html, /&lt;b&gt;raw html remains text&lt;\/b&gt;/);
  assert.match(html, /&lt;div onerror=&quot;alert\(1\)&quot;&gt;ordinary raw html&lt;\/div&gt;/);
  assert.match(html, /data-page-id="architecture"/);
  assert.doesNotMatch(html, /<\/?(?:script|style|iframe|object|img|svg|image)\b/i);
  assert.doesNotMatch(html, /evil\.example|javascript:/i);
  assert.doesNotMatch(html, /window\.__pwned|display:none/i);
  assert.doesNotMatch(html, /data:image/i);
}

// v2 导航校验 + 树形集合构建：page_id 非法/重复被剔除，白名单与 home 判定正确，
// page-meta 注释被剥离，未白名单内链不解析。
function testV2Navigation() {
  const pages = ui.validatedRepoWikiNavigation({
    navigation: {
      schema_version: 2,
      home_page_id: 'home',
      pages: [
        { page_id: 'home', title: '首页', path: 'wiki/home.md', parent_page_id: null, order: 0, children: ['a', 'b'] },
        { page_id: 'b', title: 'B 页', path: 'wiki/b.md', parent_page_id: 'home', order: 2, children: [] },
        { page_id: 'a', title: 'A 页', path: 'wiki/a.md', parent_page_id: 'home', order: 1, children: ['a1'] },
        { page_id: 'a1', title: 'A1 页', path: 'wiki/a1.md', parent_page_id: 'a', order: 1, children: [] },
        { page_id: '  ', title: '空白 id', path: 'wiki/bad.md' },  // 非法 page_id → 剔除
        { page_id: 'home', title: '重复', path: 'wiki/dup.md' },   // 重复 → 剔除
        { title: '无 id', path: 'wiki/none.md' },                  // 缺 page_id → 剔除
      ],
    },
  });
  assert.deepEqual(pages.map(p => p.page_id), ['home', 'b', 'a', 'a1']);
  assert.equal(pages.length, 4);
  // 内链白名单：集合内 id 解析为可点按钮，集合外 id 降级为禁用 span。
  const html = ui.renderSafeWikiMarkdown('[去 A](repowiki:a)\n\n[未知](repowiki:zzz)');
  assert.match(html, /data-page-id="a"/);
  assert.doesNotMatch(html, /data-page-id="zzz"/);
  // page-meta 注释剥离：首行注释不进正文。
  const stripped = ui.stripRepoWikiPageMeta('<!-- repowiki:page-meta {"page_id":"a"} -->\n# 标题\n正文');
  assert.doesNotMatch(stripped, /repowiki:page-meta/);
  assert.match(stripped, /^# 标题/);
  // 正文中段的成对标记同样剥离，但标记之间的正文必须原样保留。
  const marked = ui.stripRepoWikiPageMeta(
    '# 标题\n\n<!-- repowiki:source-baseline:start -->\n> 分支：`refs/heads/x`\n<!-- repowiki:source-baseline:end -->\n\n'
    + '<!-- repowiki:evidence-index:start -->\n- [`A`](../A.java)\n<!-- repowiki:evidence-index:end -->\n',
  );
  assert.doesNotMatch(marked, /<!--\s*repowiki:/);
  assert.match(marked, /> 分支：`refs\/heads\/x`/);
  assert.match(marked, /- \[`A`\]\(\.\.\/A\.java\)/);
}

// v2 代次新鲜度：代次自身只记生成当时的 HEAD / 图谱代次，得跟 status 的当前值比。
function testV2WikiFreshnessComparesAgainstCurrentStatus() {
  const wiki = {
    status: 'complete',
    integrity: 'healthy',
    head_commit: 'a'.repeat(40),
    graph_generation_id: 'rwgen_old_generation',
  };
  const status = {
    repository: { head_commit: 'a'.repeat(40) },
    graph: { generation_id: 'rwgen_old_generation' },
  };
  // 两项都对齐 → 最新；wikiStateLabel 必须给出「最新 · 完整」而非「已损坏」。
  assert.equal(ui.repoWikiV2Freshness(wiki, status), 'current');
  assert.equal(ui.wikiStateLabel({ ...wiki, freshness: 'current' }), '最新 · 完整');
  // 源码 HEAD 前进 → 已过期。
  assert.equal(
    ui.repoWikiV2Freshness(wiki, { ...status, repository: { head_commit: 'b'.repeat(40) } }),
    'stale',
  );
  // 图谱代次变了（分析器 ABI 升级后的确定性重建）→ 同样已过期。
  assert.equal(
    ui.repoWikiV2Freshness(wiki, { ...status, graph: { generation_id: 'rwgen_new_generation' } }),
    'stale',
  );
  assert.equal(ui.wikiStateLabel({ ...wiki, freshness: 'stale' }), '已过期');
  // 代次未完成 → damaged，这是唯一该报「已损坏」的情形。
  assert.equal(ui.repoWikiV2Freshness({ ...wiki, status: 'pending' }, status), 'damaged');
}

// 恢复命令必须跟状态卡用同一份 v2 判定。CLI 的 status 走 v1 校验器，对 v2 的 docs/ 树恒报
// damaged（它在找 v1 那 10 个固定页文件）；两边各读各的就会出现「卡片显示最新、横幅却让人工
// 处理」这种自相矛盾的界面——真机上就是这么暴露的。
// 行内渲染的占位符会嵌套：源码证据索引里每行都是 [`符号名`](路径)，行内代码先被抽成占位符，
// 链接标签于是变成占位符本身。只还原一层的话，标签位置会把占位符原样吐给用户（渲染成两个
// 私有区豆腐块夹着 RW0），符号名整个丢失——真机上整页证据索引都是这个样子。
function testNestedInlineTokensAreFullyExpanded() {
  const line = '- [`com.example.KubeUtils`](../../src/main/java/com/example/KubeUtils.java)（L1–L1314） — definition / confirmed';
  const html = ui.renderSafeWikiMarkdown(line);
  assert.doesNotMatch(html, /[]/, `渲染结果不得残留内部占位符：${html}`);
  assert.match(html, /<code>com\.example\.KubeUtils<\/code>/, `链接标签里的符号名必须还原：${html}`);
  // 非链接场景的行内代码不受影响。
  assert.match(ui.renderSafeWikiMarkdown('见 `foo()` 说明'), /<code>foo\(\)<\/code>/);
  // 多个嵌套 token 同处一行也要全部还原。
  const two = ui.renderSafeWikiMarkdown('- [`a.A`](x.java) 与 [`b.B`](y.java)');
  assert.doesNotMatch(two, /[]/);
  assert.match(two, /<code>a\.A<\/code>/);
  assert.match(two, /<code>b\.B<\/code>/);
}

function testStatusNoticeIsReadOnlyWithVersionComparison() {
  const head = '5'.repeat(40);
  const status = {
    initialization: 'initialized',
    repository: { head_commit: head },
    graph: { availability: 'ready', integrity: 'healthy', source_freshness: 'current', generation_id: 'rwgen_current_aaaa' },
    // v1 校验器对 v2 wiki 的固定误报：它在找 v1 那 10 个固定页文件，找不到就判 damaged
    wiki: { freshness: 'damaged', integrity: 'damaged', generation_id: null },
  };
  const wikiGen = {
    generation_id: 'rwwiki_ok',
    status: 'complete',
    head_commit: head,
    graph_generation_id: 'rwgen_current_aaaa',
    generated_at: '2020-01-01T00:00:00+00:00',
  };

  // 一切对得上 → 不报警。这条钉死 v1 校验器的 damaged 不得再冒出来。
  assert.equal(ui.repoWikiStatusNotice(status, { wiki: wikiGen }), null);

  // 图谱重建（代码没变）→ 一行说清，且给出版本比对 + 生成日期。
  const graphStale = { wiki: { ...wikiGen, graph_generation_id: 'rwgen_old_bbbb' } };
  const notice = ui.repoWikiStatusNotice(status, graphStale);
  assert.equal(notice.kind, 'warn');
  assert.match(notice.message, /2020/, `要带生成日期：${notice.message}`);
  assert.match(notice.message, /old_bbbb|old/, `要带原版本：${notice.message}`);
  assert.match(notice.message, /current_aaaa|current/, `要带当前版本：${notice.message}`);
  assert.match(notice.message, /代码图谱/, `代码没变时应说是图谱重建：${notice.message}`);
  assert.doesNotMatch(notice.message, /源码从/, '源码没变就不该说源码变了');

  // 源码前进 → 说源码变了，不说图谱。
  const headStale = { wiki: { ...wikiGen, head_commit: 'e'.repeat(40) } };
  const headNotice = ui.repoWikiStatusNotice(status, headStale);
  assert.match(headNotice.message, /源码从/);
  assert.doesNotMatch(headNotice.message, /代码图谱/);

  // Dashboard 只做查看：任何提示都不得出现操作指引 / 命令。
  for (const message of [notice.message, headNotice.message]) {
    assert.doesNotMatch(message, /repowiki |--repo|--data-root|复制|执行|请运行/, `不得给操作指引：${message}`);
  }
}

// 「人工处理命令」面板已移除：它给的是 v1 命令（对 v2 wiki 无效）、路径是占位符（复制也跑不了），
// 且过期时只给 status（查一遍，修不了）。Dashboard 是查看器，不猜命令。
function testManualCommandPanelIsGone() {
  for (const name of ['repoWikiManualCommand', 'repoWikiRecoveryCommands', 'repoWikiRecoveryNotice']) {
    assert.equal(ui[name], undefined, `${name} 应已随命令面板一起移除`);
  }
  const source = fs.readFileSync(appPath, 'utf8');
  assert.doesNotMatch(source, /REPOWIKI_MANUAL_COMMANDS/, 'app.js 不得再有写死的人工命令表');
  assert.doesNotMatch(source, /只复制，不执行/, '命令面板文案应已删除');
  assert.doesNotMatch(source, /data-repowiki-copy-command|repowikiCopyCommand/, '复制按钮及其处理器应已删除');
  const css = fs.readFileSync(path.join(__dirname, '..', 'dist', 'app.css'), 'utf8');
  assert.doesNotMatch(css, /repowiki-manual/, '命令面板样式应已删除');
}

function testRegistrySelectorsAreOpaque() {
  const entries = ui.repoWikiSelectorEntries({
    shared: [
      { project_id: '123e4567-e89b-12d3-a456-426614174000', name: 'shared', machines: { mac: '/secret/shared' } },
      { project_id: '123e4567-e89b-12d3-a456-426614174001', name: 'foreign', machines: { win: 'F:\\secret\\foreign' } },
    ],
    private: [{ name: 'private-exact', path: '/secret/private', git_url: 'ssh://example' }],
  }, 'mac');
  assert.deepEqual(entries.map(item => item.selector), [
    { kind: 'shared', projectId: '123e4567-e89b-12d3-a456-426614174000' },
    { kind: 'private', name: 'private-exact' },
  ]);
  assert.deepEqual(entries.map(item => item.key), ['rw-project-0', 'rw-project-1']);
  assert.doesNotMatch(JSON.stringify(entries), /\/secret\//);
}

function testEnvelopeValidation() {
  const result = ui.unwrapRepoWikiEnvelope({
    protocol_version: 1,
    command: 'status',
    ok: true,
    result: { wiki: { freshness: 'current' } },
    warnings: [],
  }, 'status');
  assert.equal(result.wiki.freshness, 'current');
  assert.throws(() => ui.unwrapRepoWikiEnvelope({
    protocol_version: 1,
    command: 'status',
    ok: false,
    result: {},
    warnings: [],
    error: { code: 'wiki_absent', message: '<b>not generated</b>', details: {} },
  }, 'status'), /wiki_absent/);
  assert.throws(() => ui.unwrapRepoWikiEnvelope({
    protocol_version: 2,
    command: 'status',
    ok: true,
    result: {},
    warnings: [],
  }, 'status'), /协议/);
}

function testRepoWikiUsesNativeKeyboardControls() {
  const html = fs.readFileSync(path.join(__dirname, '..', 'dist', 'index.html'), 'utf8');
  assert.match(html, /<button[^>]+data-tab="repowiki"/);
  assert.match(html, /<button[^>]+id="btn-repowiki-refresh"/);
  assert.match(html, /<form[^>]+id="repowiki-graph-search-form"/);
  assert.match(html, /<button[^>]+type="submit"[^>]*>搜索<\/button>/);
  assert.match(html, /<nav[^>]+class="[^"]*repowiki-page-nav/);
  assert.match(html, /<div[^>]+id="repowiki-page-nav"/);
}

function testNoAutomaticRepoWikiCalls() {
  const original = fs.readFileSync(appPath, 'utf8');
  for (const lineEnding of ['\n', '\r\n']) {
  const source = original.replace(/\r\n/g, '\n').replace(/\n/g, lineEnding).replace(/\r\n/g, '\n');
  const refreshAll = source.match(/async function refreshAll\(\) \{([\s\S]*?)\n\}\n\n\/\/ 拉分页/);
  assert.ok(refreshAll, 'refreshAll source block should be found');
  assert.doesNotMatch(refreshAll[1], /invoke\(['"]repowiki_/);
  const bootstrap = source.match(/DOMContentLoaded[\s\S]*?\n\s*\}\);/);
  assert.ok(bootstrap, 'bootstrap block should be found');
  assert.doesNotMatch(bootstrap[0], /invoke\(['"]repowiki_/);
  const invoked = [...source.matchAll(/invoke\(['"](repowiki_[a-z_]+)['"]/g)].map(match => match[1]);
  assert.deepEqual([...new Set(invoked)].sort(), [
    'repowiki_graph_search',
    'repowiki_read_generations',
    'repowiki_read_page',
    'repowiki_read_project',
    'repowiki_status',
  ]);
  for (const command of ['repowiki_graph_show', 'repowiki_graph_callers', 'repowiki_graph_callees', 'repowiki_graph_impact']) {
    assert.match(source, new RegExp(`['"]${command}['"]`));
  }
  assert.doesNotMatch(source, /invoke\([^\n]*(?:repowiki_(?:build|update|prepare|publish|review)|graph build|graph update)/i);
  const loadProject = source.match(/async function loadRepoWikiProject\(\) \{([\s\S]*?)\n\}\n\nfunction renderRepoWikiFailure/);
  assert.ok(loadProject, 'loadRepoWikiProject source block should be found');
  assert.doesNotMatch(loadProject[1], /loadRepoWikiPage\(/, '项目概览不得自动读取正文页面');

  const settings = source.match(/async function loadSettings\(\) \{([\s\S]*?)\n\}\n\n\/\/ -------- Utils/);
  assert.ok(settings, 'settings source block should be found');
  assert.match(settings[1], /invoke\('update_repowiki_settings'/);
  }
}

async function testMockContractCoverage() {
  const selector = { kind: 'private', name: 'company-x' };
  const calls = [
    ['repowiki_status', { selector }, 'status'],
    ['repowiki_read_project', { selector }, 'read project'],
    ['repowiki_read_page', { selector, pageId: 'architecture' }, 'read page'],
    ['repowiki_read_generations', { selector }, 'read generations'],
    ['repowiki_graph_search', { selector, text: 'process', limit: 20 }, 'graph search'],
    ['repowiki_graph_show', { selector, nodeId: 'a'.repeat(64) }, 'graph show'],
    ['repowiki_graph_callers', { selector, nodeId: 'a'.repeat(64), depth: 2, limit: 50 }, 'graph callers'],
    ['repowiki_graph_callees', { selector, nodeId: 'a'.repeat(64), depth: 2, limit: 50 }, 'graph callees'],
    ['repowiki_graph_impact', { selector, nodeId: 'a'.repeat(64), depth: 2, limit: 50 }, 'graph impact'],
  ];
  for (const [command, args, expected] of calls) {
    const envelope = await ui.mockInvoke(command, args);
    assert.equal(envelope.protocol_version, 1, command);
    assert.equal(envelope.command, expected, command);
    assert.equal(envelope.ok, true, command);
  }
}

// v2 正文渲染回归位：GFM 表格此前完全没渲染（管道符原样露出），
// 站内链接此前只认 `repowiki:` / `#` 前缀，而 v2 正文写的是相对 .md 路径，
// 导致整站内部跳转失效且被画上删除线。
function testMarkdownTablesAndRelativeLinks() {
  ui.validatedRepoWikiNavigation({
    navigation: {
      schema_version: 2,
      home_page_id: 'home',
      pages: [
        { page_id: 'home', title: '首页', path: 'docs/00-从这里开始.md', parent_page_id: null, order: 0 },
        { page_id: 'arch', title: '系统架构', path: 'docs/01-项目全貌/01-系统架构.md', parent_page_id: 'home', order: 1 },
        { page_id: 'mod', title: '模块', path: 'docs/02-核心模块/01-ai/00-本节概览.md', parent_page_id: 'home', order: 2 },
      ],
    },
  });

  const table = ui.renderSafeWikiMarkdown([
    '| 层 | 包 | 职责 |',
    '| --- | --- | --- |',
    '| 模型接入 | pi-ai | `types.ts` 统一协议 |',
  ].join('\n'));
  assert.match(table, /<table class="repowiki-table">/);
  assert.match(table, /<th>层<\/th>/);
  assert.match(table, /<td><code>types\.ts<\/code> 统一协议<\/td>/);
  assert.doesNotMatch(table, /<p>\|/);

  // 缺分隔行不算表格：按段落原样吐回，不得吞掉内容。
  const notTable = ui.renderSafeWikiMarkdown('| 只有一行 | 没有分隔行 |');
  assert.doesNotMatch(notTable, /<table/);
  assert.match(notTable, /只有一行/);

  // 站内相对链接：同级、跨层级 `../`、URL 编码都要落回页面。
  assert.equal(ui.repoWikiPageIdFromLink('01-项目全貌/01-系统架构.md', 'docs/00-从这里开始.md'), 'arch');
  assert.equal(
    ui.repoWikiPageIdFromLink('../../01-项目全貌/01-系统架构.md', 'docs/02-核心模块/01-ai/00-本节概览.md'),
    'arch',
  );
  const encoded = `01-${encodeURIComponent('项目全貌')}/01-${encodeURIComponent('系统架构')}.md`;
  assert.equal(ui.repoWikiPageIdFromLink(encoded, 'docs/00-从这里开始.md'), 'arch');
  // 锚点只影响页内定位，不该妨碍页面解析。
  assert.equal(ui.repoWikiPageIdFromLink('01-项目全貌/01-系统架构.md#分层', 'docs/00-从这里开始.md'), 'arch');

  // 源码文件、站外地址、协议链接、越过 .repowiki 根：一律不可点。
  assert.equal(
    ui.repoWikiPageIdFromLink('../../../packages/ai/src/types.ts', 'docs/02-核心模块/01-ai/00-本节概览.md'),
    null,
  );
  assert.equal(ui.repoWikiPageIdFromLink('https://example.com/page', 'docs/00-从这里开始.md'), null);
  assert.equal(ui.repoWikiPageIdFromLink('javascript:alert(1)', 'docs/00-从这里开始.md'), null);
  assert.equal(ui.repoWikiPageIdFromLink('../../../../etc/passwd', 'docs/00-从这里开始.md'), null);

  // 指向源码的引用渲染成中性样式，不再画删除线。
  const body = ui.renderSafeWikiMarkdown('见 [`types.ts`](../../../packages/ai/src/types.ts)');
  assert.match(body, /wiki-external-ref/);
  assert.doesNotMatch(body, /wiki-disabled-link/);
}

// mermaid graph 子集渲染：能画的画成 SVG，画不了的必须原样回落成代码块。
function testMermaidFlowchartRendering() {
  const diagram = ui.renderSafeWikiMarkdown([
    '```mermaid',
    'graph TD',
    '  CLI["pi 命令"] --> CA["pi-coding-agent<br/>548 文件"]',
    '  SRV["pi-server"] -.->|"起子进程"| CA',
    '```',
  ].join('\n'));
  assert.match(diagram, /<div class="repowiki-diagram">/);
  assert.doesNotMatch(diagram, /<pre><code>graph TD/);
  assert.equal((diagram.match(/class="rw-diagram-node"/g) || []).length, 3);
  assert.equal((diagram.match(/stroke-dasharray/g) || []).length, 1); // 仅 -.-> 那条
  assert.match(diagram, /起子进程/);
  assert.match(diagram, /<tspan[^>]*>548 文件<\/tspan>/); // <br/> 折成多行

  // 节点标签必须转义，不能把 wiki 内容当 HTML 注进 SVG。
  const injected = ui.renderSafeWikiMarkdown([
    '```mermaid',
    'graph TD',
    '  A["<script>alert(1)</script>"] --> B["安全"]',
    '```',
  ].join('\n'));
  assert.doesNotMatch(injected, /<script/i);

  // 子集外语法一律回落成代码块，不吞内容、不画错图。
  for (const source of ['sequenceDiagram\n  A->>B: hi', 'graph TD\n  subgraph 分组\n  end', 'classDiagram\n  A <|-- B']) {
    const fallback = ui.renderSafeWikiMarkdown('```mermaid\n' + source + '\n```');
    assert.match(fallback, /<pre><code>/);
    assert.match(fallback, new RegExp(source.split('\n')[0]));
    assert.doesNotMatch(fallback, /repowiki-diagram/);
  }

  // 非 mermaid 围栏不受影响。
  assert.match(ui.renderSafeWikiMarkdown('```js\nconst a = 1;\n```'), /<pre><code>const a = 1;<\/code><\/pre>/);
}

async function main() {
  await testSafeMarkdown();
  testMarkdownTablesAndRelativeLinks();
  testMermaidFlowchartRendering();
  testV2Navigation();
  testV2WikiFreshnessComparesAgainstCurrentStatus();
  testStatusNoticeIsReadOnlyWithVersionComparison();
  testManualCommandPanelIsGone();
  testNestedInlineTokensAreFullyExpanded();
  testRegistrySelectorsAreOpaque();
  testEnvelopeValidation();
  testRepoWikiUsesNativeKeyboardControls();
  testNoAutomaticRepoWikiCalls();
  await testMockContractCoverage();
  console.log('repowiki-ui.test.js: ok');
}

main().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
