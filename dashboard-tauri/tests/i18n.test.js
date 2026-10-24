const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const file = path.join(__dirname, '../dist/i18n.js');
function api(options = {}) {
  assert.ok(fs.existsSync(file), '需要随包提供独立的离线语言模块');
  return require(file).createI18n(options);
}
function storage(initial) {
  const values = new Map(initial ? [['project-dashboard.language', initial]] : []);
  return {values, getItem:key => values.get(key) ?? null, setItem:(key,value) => values.set(key,value)};
}
test('语言默认中文，只有有效的本机偏好恢复英文', () => {
  assert.equal(api().language, 'zh-CN');
  assert.equal(api({storage:storage('en')}).language, 'en');
  for (const invalid of ['fr', 'EN', '{broken}', '']) assert.equal(api({storage:storage(invalid)}).language, 'zh-CN');
});
test('切换即时通知并保存；拒绝不支持的语言', () => {
  const saved = storage(), changes = [];
  const ui = api({storage:saved,onChange:lang => changes.push(lang)});
  assert.deepEqual(ui.setLanguage('en'), {saved:true});
  assert.equal(ui.t('会话看板'), 'Session Board');
  assert.equal(saved.values.get('project-dashboard.language'), 'en');
  assert.deepEqual(changes, ['en']);
  assert.equal(api({storage:saved}).language, 'en');
  assert.throws(() => ui.setLanguage('fr'), /Unsupported/);
  ui.setLanguage('zh-CN');
  assert.equal(ui.t('会话看板'), '会话看板');
});
test('插值只替换一轮，不翻译数据，也不把美元符号当替换指令', () => {
  const ui = api({storage:storage('en')});
  assert.equal(ui.t('第 {0}-{1} 条 / 共 {2} 条', 21,40,143), '21–40 of 143');
  const userTitle = '会话看板 $& {1} <script>';
  assert.equal(ui.t('未知判定（{0}）', userTitle), 'Unknown status (' + userTitle + ')');
  assert.equal(ui.t('这是一个未登记的用户标题'), '这是一个未登记的用户标题');
  assert.equal(ui.html`<span>${'会话看板'}</span>`, '<span>会话看板</span>');
});
test('读写本机偏好被拒绝时仍可使用，返回无法保存的状态', () => {
  const denied = {getItem(){throw new Error('denied');},setItem(){throw new Error('denied');}};
  const ui = api({storage:denied});
  assert.equal(ui.language, 'zh-CN');
  assert.deepEqual(ui.setLanguage('en'), {saved:false});
  assert.equal(ui.language, 'en');
  assert.equal(ui.t('设置'), 'Settings');
});
test('静态翻译只更新明确标记的文本与属性，不改输入值和未标记正文', () => {
  const ui = api({storage:storage('en')});
  const node = (attrs, text) => ({attrs,textContent:text,value:'会话看板',getAttribute(key){return this.attrs[key] ?? null;},setAttribute(key,value){this.attrs[key]=value;}});
  const marked = node({'data-i18n':'会话看板','data-i18n-title':'设置'},'会话看板');
  const content = node({},'会话看板');
  const resolved = node({'data-i18n':'读取身份中…'},'mbp-real-machine');
  require(file).applyTranslations({querySelectorAll:()=>[marked,content,resolved]},ui);
  assert.equal(marked.textContent,'Session Board'); assert.equal(marked.attrs.title,'Settings');
  assert.equal(marked.value,'会话看板'); assert.equal(content.textContent,'会话看板');
  assert.equal(resolved.textContent,'mbp-real-machine');
});

test('英文词典完整保留消息参数，静态标记全部有英文', () => {
  const {EN} = require(file);
  const slots = value => [...new Set([...value.matchAll(/\{(\d+)\}/g)].map(m => m[1]))].sort();
  for (const [key,value] of Object.entries(EN)) {
    assert.deepEqual(slots(value),slots(key),key.slice(0,100));
    assert.doesNotMatch(value.replace(/<!--[\s\S]*?-->/g,''),/\p{Script=Han}/u,key.slice(0,100));
  }
  const html = fs.readFileSync(path.join(__dirname,'../dist/index.html'),'utf8');
  const decode = text => text.replace(/&quot;/g,'"').replace(/&#x27;/g,"'").replace(/&amp;/g,'&').replace(/&lt;/g,'<').replace(/&gt;/g,'>');
  for (const m of html.matchAll(/data-i18n(?:-(?:title|placeholder|aria-label))?="([^"]+)"/g)) assert.ok(Object.hasOwn(EN,decode(m[1])),m[1]);
});

test('英文动态同步、判定和 Wiki 状态正确，正文和未知诊断保留原文', () => {
  const vm = require('node:vm');
  const locale = api({storage:storage('en')});
  const context = vm.createContext({window:{DashboardI18n:locale},console,module:{exports:{}}});
  vm.runInContext(fs.readFileSync(path.join(__dirname,'../dist/app.js'),'utf8'),context);
  const call = code => vm.runInContext(code,context);
  assert.equal(call('computeSyncStatusView(null,0).text'),'Cross-machine sync: no sync history');
  assert.equal(call("computeSyncStatusView({notice:'已本地保存·推送失败·下次重试'},0).text"),'Saved locally · Push failed · Retry next time');
  assert.equal(call("verdictLabel({kind:'ahead',latest_machine_id:'主机A',behind_machine_id:'主机B'})"),'主机A is ahead of 主机B');
  assert.equal(call("wikiStateLabel({freshness:'current',integrity:'healthy'})"),'Current · Verified');
  assert.equal(call(String.raw`renderSafeWikiMarkdown('# 会话看板\n\n这是用户原文')`).includes('这是用户原文'),true);
  assert.equal(call("computeSyncStatusView({error:'底层诊断原文'},0).text"),'底层诊断原文');
  locale.setLanguage('zh-CN');
  assert.equal(call("verdictLabel({kind:'synced'})"),'同步');
});
