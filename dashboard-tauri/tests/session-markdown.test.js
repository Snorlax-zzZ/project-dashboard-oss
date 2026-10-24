const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');

const source = fs.readFileSync(path.join(__dirname, '../dist/app.js'), 'utf8');
const marked = require('../dist/marked.min.js');

// 执行真实 app.js 和随包 Marked，不用替代解析器模拟安全行为。
function load(markdown) {
  if (arguments.length === 0) markdown = marked;
  const context = vm.createContext({ window: { marked: markdown }, console });
  vm.runInContext(source, context);
  return context;
}

const ui = load();

test('按正式页面顺序加载真实会话解析脚本', () => {
  const dist = path.join(__dirname, '../dist');
  const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
  const scripts = [...html.matchAll(/<script\s+src="\.\/([^\"]+)"\s*><\/script>/g)].map(match => match[1]);
  assert.ok(scripts.indexOf('marked.min.js') >= 0);
  assert.ok(scripts.indexOf('marked.min.js') < scripts.indexOf('app.js'));
  const page = vm.createContext({ console });
  page.window = page;
  for (const script of scripts.filter(name => ['marked.min.js', 'app.js'].includes(name))) {
    vm.runInContext(fs.readFileSync(path.join(dist, script), 'utf8'), page);
  }
  assert.equal(typeof page.marked.Marked, 'function');
  assert.match(page.renderDialogMarkdown(['# 标题']), /<h1>标题<\/h1>/);
  assert.doesNotMatch(page.renderDialogMarkdown(['[详情](javascript:example)']), /<a\b/i);
});

test('链接地址中的引号和尖括号不能注入属性或节点', () => {
  const html = ui.renderDialogMarkdown(['[引用](<https://example.com/"onmouseover="example>) [单引号](https://example.com/\'example)']);
  assert.match(html, /&quot;onmouseover=&quot;/);
  assert.match(html, /&#39;/);
  assert.doesNotMatch(html, /href="[^"]*"\s+on\w+=|<script\b/i);
});

const dangerous = [
  'javascript:globalThis.__sessionProbe=1',
  'JaVaScRiPt:globalThis.__sessionProbe=1',
  'javascript:alert%281%29',
  'java&#x73;cript:globalThis.__sessionProbe=1',
  'javascript&#58;globalThis.__sessionProbe=1',
  '&#106;avascript:globalThis.__sessionProbe=1',
  'java&Tab;script:globalThis.__sessionProbe=1',
  'java&#10;script:globalThis.__sessionProbe=1',
  'data:text/html,example',
  'vbscript:example',
  'file:///tmp/example.txt',
  'tauri://localhost/index.html',
  'repowiki:home',
  '//example.invalid/path',
  './index.html',
  '#section',
  '%6aavascript:example',
];

for (const href of dangerous) {
  test(`会话正文不创建危险或未授权地址链接：${href}`, () => {
    const html = ui.renderDialogMarkdown([`🧑 [查看详情](${href})`]);
    assert.match(html, /查看详情/);
    assert.doesNotMatch(html, /<a\b|\bhref\s*=/i);
  });
}

test('引用链接与嵌套标签同样受限', () => {
  const html = ui.renderDialogMarkdown(['🤖 [**详情**][ref]\n\n[ref]: javascript:globalThis.__sessionProbe=1']);
  assert.match(html, /<strong>详情<\/strong>/);
  assert.doesNotMatch(html, /<a\b|\bhref\s*=/i);
});

test('会话图片只显示替代文字，不创建任何资源节点', () => {
  const html = ui.renderDialogMarkdown(['🤖 ![远程图片](https://example.invalid/image.png)\n\n![本地图片](./logo.svg)\n\n[![嵌套图片](data:image/svg+xml,example)](javascript:example)']);
  for (const label of ['远程图片', '本地图片', '嵌套图片']) assert.match(html, new RegExp(label));
  assert.doesNotMatch(html, /<(?:img|svg|iframe|object|embed|a)\b|\b(?:href|src)\s*=/i);
});

test('原始 HTML、事件属性和实体编码不能变成可执行节点', () => {
  const html = ui.renderDialogMarkdown(['🧑 <script>globalThis.__sessionProbe=1</script>\n\n<img src=x onerror="globalThis.__sessionProbe=1">\n\n<svg onload="globalThis.__sessionProbe=1"></svg>\n\n<a href="javascript:example">原始链接</a>\n\n&#60;img src=x onerror=example&#62;']);
  assert.match(html, /原始链接/);
  assert.doesNotMatch(html, /<(?:script|img|svg|iframe|object|embed|a)\b/i);
});

test('安全链接保留文本格式并转义属性', () => {
  const html = ui.renderDialogMarkdown(['🤖 [**官网**](https://example.com/path?q=1&next=2 "标题") [HTTP](http://example.com/) [邮件](mailto:demo@example.com)']);
  assert.match(html, /<a\b[^>]*href="https:\/\/example.com\/path\?q=1&amp;next=2"[^>]*><strong>官网<\/strong><\/a>/);
  assert.match(html, /href="http:\/\/example.com\/"/);
  assert.match(html, /href="mailto:demo@example.com"/);
  assert.equal((html.match(/<a\b/g) || []).length, 3);
});

test('标题、列表、表格、代码和消息角色保持可读', () => {
  const html = ui.renderDialogMarkdown(['🧑 # 标题\n\n- 列表\n\n| 字段 | 值 |\n| --- | --- |\n| 状态 | **正常** |\n\n```html\n<div>示例</div>\n```', '🤖 `const value = 1`']);
  for (const fragment of ['<h1>标题</h1>', '<li>列表</li>', '<table>', '<strong>正常</strong>', '<pre><code', 'msg-user', 'msg-asst', '<code>const value = 1</code>']) assert.ok(html.includes(fragment), fragment);
  assert.match(html, /&lt;div&gt;示例&lt;\/div&gt;/);
  assert.doesNotMatch(html, /<div>示例/);
});

test('未加载或解析失败时安全回退，不输出活动链接或 HTML', () => {
  for (const markdown of [undefined, {}, { Marked: class { parse() { throw new Error('synthetic parse failure'); } }, parse() { throw new Error('synthetic parse failure'); } }]) {
    const html = load(markdown).renderDialogMarkdown(['🤖 <img src=x>\n[详情](javascript:example)']);
    assert.doesNotMatch(html, /<(?:a|img)\b/i);
    assert.match(html, /&lt;img src=x&gt;/);
  }
});

test('会话渲染不修改共享 Marked 默认配置', () => {
  const markdown = '[示例](https://example.com/)';
  const before = marked.parse(markdown);
  ui.renderDialogMarkdown([markdown]);
  assert.equal(marked.parse(markdown), before);
});
