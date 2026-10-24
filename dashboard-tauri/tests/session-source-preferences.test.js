const test = require('node:test');
const assert = require('node:assert/strict');
const ui = require('../dist/app.js');

test('来源偏好缺失或错误时回退默认，去除未知重复 ID 并追加新来源', () => {
  const defaults = {version:1,order:[...ui.SESSION_SOURCE_IDS],hidden:[]};
  assert.deepEqual(ui.normalizeSessionSourcePreferences(null), defaults);
  assert.deepEqual(ui.normalizeSessionSourcePreferences({version:2,order:['codex'],hidden:[]}), defaults);
  const value = ui.normalizeSessionSourcePreferences({version:1,order:['codex','unknown','codex','pi'],hidden:['unknown','pi','pi']});
  assert.deepEqual(value.order, ['codex','pi',...ui.SESSION_SOURCE_IDS.filter(id=>!['codex','pi'].includes(id))]);
  assert.deepEqual(value.hidden, ['pi']);
  const repaired = ui.normalizeSessionSourcePreferences({version:1,order:[...ui.SESSION_SOURCE_IDS],hidden:[...ui.SESSION_SOURCE_IDS]});
  assert.equal(ui.visibleSessionSources(repaired).length, 1);
});

test('保存后重启按原可见顺序恢复，存储异常明确失败且不覆盖已有配置', () => {
  const values = new Map();
  const storage = {getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
  const prefs = ui.normalizeSessionSourcePreferences({version:1,order:['zcode','codex'],hidden:['claude-cli','pi','grok']});
  ui.persistSessionSourcePreferences(prefs,storage);
  assert.deepEqual(ui.readSessionSourcePreferences(storage),prefs);
  assert.equal(ui.visibleSessionSources(ui.readSessionSourcePreferences(storage))[0],'zcode');
  assert.throws(()=>ui.persistSessionSourcePreferences(prefs,{setItem(){throw new Error('quota');}}),/quota/);
  assert.throws(()=>ui.persistSessionSourcePreferences({...prefs,hidden:[...ui.SESSION_SOURCE_IDS]},storage),/至少/);
  assert.deepEqual(ui.readSessionSourcePreferences(storage),prefs);
  assert.equal(ui.visibleSessionSources(ui.readSessionSourcePreferences({getItem(){throw new Error('denied');}})).length,9);
  assert.equal(ui.visibleSessionSources(ui.readSessionSourcePreferences({getItem:()=>'{broken json'})).length,9);
});

test('上下移动改变顺序但不改变显示选择，不允许越界或修改原草稿', () => {
  const prefs = ui.normalizeSessionSourcePreferences({version:1,order:['codex','pi'],hidden:['pi']});
  const moved = ui.moveSessionSource(prefs,'pi',-1);
  assert.deepEqual(moved.order.slice(0,2),['pi','codex']);
  assert.deepEqual(moved.hidden,['pi']);
  assert.deepEqual(prefs.order.slice(0,2),['codex','pi']);
  assert.deepEqual(ui.moveSessionSource(moved,'pi',-1),moved);
  assert.deepEqual(ui.moveSessionSource(moved,'unknown',1),moved);
});

test('拖拽可插入目标前后，双向移动保留其他项顺序和勾选，取消目标不改变草稿', () => {
  const prefs = ui.normalizeSessionSourcePreferences({version:1,order:['codex','pi','grok','zcode'],hidden:['pi']});
  const after = ui.reorderSessionSource(prefs,'codex','grok','after');
  assert.deepEqual(after.order.slice(0,4),['pi','grok','codex','zcode']);
  const before = ui.reorderSessionSource(prefs,'zcode','codex','before');
  assert.deepEqual(before.order.slice(0,4),['zcode','codex','pi','grok']);
  assert.deepEqual(after.hidden,['pi']);
  assert.deepEqual(before.hidden,['pi']);
  assert.deepEqual(prefs.order.slice(0,4),['codex','pi','grok','zcode']);
  for(const [source,target,position] of [['codex','codex','after'],['unknown','pi','before'],['pi','unknown','after'],['codex','pi','invalid']]) {
    assert.deepEqual(ui.reorderSessionSource(prefs,source,target,position),prefs);
  }
});
