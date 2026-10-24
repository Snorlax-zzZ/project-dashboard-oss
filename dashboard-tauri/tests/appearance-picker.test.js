const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function launch(saved = new Map()) {
  const events = new Map(), nodes = new Map(), values = new Map(saved);
  const document = { readyState: 'complete', documentElement: { dataset: {} },
    getElementById: id => nodes.get(id) || null, addEventListener: (type, fn) => events.set(type, fn) };
  function node(id, palette) {
    const n = { id, hidden: id === 'appearance-picker', dataset: palette ? { paletteOption: palette } : {},
      attrs: {}, style: {}, handlers: {}, offsetHeight: 236, offsetWidth: 264,
      classList: { toggle() {} }, setAttribute(k, v) { this.attrs[k] = v; },
      addEventListener(type, fn) { this.handlers[type] = fn; }, focus() { document.activeElement = this; },
      contains(target) { return target === this || (this.children || []).includes(target); },
      closest() { return palette ? this : null; }, getBoundingClientRect: () => ({ left: 28, top: 680, bottom: 724 }) };
    nodes.set(id, n); return n;
  }
  for (const id of ['btn-theme-toggle', 'appearance-label', 'appearance-theme-name', 'btn-appearance-picker', 'appearance-picker']) node(id);
  const options = ['obsidian', 'alabaster', 'nord', 'graphite'].map(p => node(p, p));
  const panel = nodes.get('appearance-picker'); panel.children = options;
  panel.querySelector = () => options.find(n => n.attrs['aria-pressed'] === 'true');
  document.querySelectorAll = () => options;
  const window = { innerWidth: 1200, innerHeight: 800, addEventListener() {}, matchMedia: () => ({ matches: true }),
    localStorage: { getItem: k => values.get(k) || null, setItem: (k, v) => values.set(k, v) } };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../dist/appearance.js'), 'utf8'), { document, window });
  return { nodes, events, values, document, window, panel, trigger: nodes.get('btn-appearance-picker') };
}

test('主题弹层支持选择即收起、Esc 返回入口、外部点击和焦点离开关闭', () => {
  const app = launch();
  app.trigger.handlers.click();
  assert.equal(app.panel.hidden, false);
  assert.equal(app.trigger.attrs['aria-expanded'], 'true');
  assert.equal(app.document.activeElement.dataset.paletteOption, 'obsidian');
  app.events.get('click')({ target: app.nodes.get('nord') });
  assert.equal(app.document.documentElement.dataset.palette, 'nord');
  assert.equal(app.panel.hidden, true);
  assert.equal(app.document.activeElement, app.trigger);
  app.trigger.handlers.click();
  app.events.get('keydown')({ key: 'Escape', preventDefault() {} });
  assert.equal(app.panel.hidden, true);
  assert.equal(app.document.activeElement, app.trigger);
  app.trigger.handlers.click();
  app.events.get('click')({ target: { closest: () => null } });
  assert.equal(app.panel.hidden, true);
  app.trigger.handlers.click();
  app.events.get('focusin')({ target: {} });
  assert.equal(app.panel.hidden, true);
});

test('浅色重启后切回深色仍恢复之前选择的色系', () => {
  const first = launch();
  first.window.__setPalette('nord');
  first.nodes.get('btn-theme-toggle').handlers.click();
  const restarted = launch(first.values);
  assert.equal(restarted.document.documentElement.dataset.palette, 'alabaster');
  restarted.nodes.get('btn-theme-toggle').handlers.click();
  assert.equal(restarted.document.documentElement.dataset.palette, 'nord');
  assert.equal(restarted.nodes.get('appearance-theme-name').textContent, '钛金冰蓝');
});
