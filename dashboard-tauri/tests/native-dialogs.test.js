// 防回归 · 静态扫描：dist/app.js 不得出现浏览器原生对话框调用（confirm / alert / prompt）。
//
// 命题背景——Wry 0.55.1 macOS 未实现原生 confirm() 面板：点击哑弹、立即返回 undefined，
// 于是「确认」分支永远走不到，功能静默失效。这个坑的致命特征是**自动化测试全绿、只有真机 GUI 才暴露**，
// 项目已因此踩过两次（第一次：删除 / 关联 / 双形态确认；第二次：身份迁移执行按钮）。
// 正确做法是走本文件同目录 dist/app.js 里封装的 uiConfirm()（DOM 弹窗 + Promise）。
//
// 扫描前先剥注释，否则 uiConfirm 上方那段解释坑的注释本身会被误判成违规。
// 为证明扫描器不是空转，另有两组 fixture：一组必须全部命中，一组必须零命中。
//
// 运行：node dashboard-tauri/tests/native-dialogs.test.js （期望末尾打印 native-dialogs.test.js: ok）

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const APP_JS = path.join(__dirname, '..', 'dist', 'app.js');

// 裸调用：前面不能是 . 或标识符字符，避免把 uiConfirm( / obj.confirm( 里的合法命名算进来
const BARE_CALL = /(?<![.\w$])(?:confirm|alert|prompt)\s*\(/g;
// 全局对象显式调用：window.confirm( / globalThis.alert( / self.prompt(
const GLOBAL_CALL = /\b(?:window|globalThis|self)\s*\.\s*(?:confirm|alert|prompt)\s*\(/g;

// 剥注释但保留行号：块注释按原换行数补回，行注释置空（跳过 :// 免误伤 URL）。
function stripComments(source) {
  const withoutBlock = source.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '));
  return withoutBlock
    .split('\n')
    .map((line) => {
      const idx = line.search(/(?<!:)\/\//);
      return idx === -1 ? line : line.slice(0, idx);
    })
    .join('\n');
}

// 返回 [{ line, snippet }]，line 为 1 起的行号。
// 两条正则会在 `globalThis . alert (` 这类写法上同时命中（一条从 globalThis 起、一条从 alert 起），
// 按匹配结束位置（都停在同一个左括号）去重，避免同一处违规被数成两处。
function scanNativeDialogCalls(source) {
  const code = stripComments(source);
  const byCallSite = new Map();
  for (const pattern of [BARE_CALL, GLOBAL_CALL]) {
    pattern.lastIndex = 0;
    let match;
    while ((match = pattern.exec(code)) !== null) {
      const callSite = match.index + match[0].length;
      const line = code.slice(0, match.index).split('\n').length;
      const prev = byCallSite.get(callSite);
      // 同一处保留更长的匹配（含 window./globalThis. 前缀那条），报错信息更完整
      if (!prev || prev.snippet.length < match[0].length) {
        byCallSite.set(callSite, { line, snippet: match[0].trim() });
      }
    }
  }
  return [...byCallSite.values()].sort((a, b) => a.line - b.line);
}

// 主命题：真实 dist/app.js 零原生对话框调用。
function testAppJsHasNoNativeDialogs() {
  const source = fs.readFileSync(APP_JS, 'utf8');
  const hits = scanNativeDialogCalls(source);
  const detail = hits.map((h) => `  app.js:${h.line}  ${h.snippet}`).join('\n');
  assert.equal(
    hits.length,
    0,
    `dist/app.js 出现原生对话框调用（Wry macOS 下哑弹、功能静默失效），改用 uiConfirm()：\n${detail}`,
  );
}

// 反向自证：扫描器必须能抓到各种写法，否则上面那条断言是空转。
function testScannerCatchesViolations() {
  const bad = [
    "if (!confirm('执行吗？')) return;",
    'window.confirm("x");',
    'alert(msg);',
    'globalThis . alert ( msg );',
    'const v = prompt("name");',
    'self.prompt("name");',
  ].join('\n');
  const hits = scanNativeDialogCalls(bad);
  assert.equal(hits.length, 6, `六种违规写法必须全部命中，实际命中 ${hits.length} 处`);
  assert.deepEqual(hits.map((h) => h.line), [1, 2, 3, 4, 5, 6]);
}

// 反向自证：合法写法与注释不得误报，否则这条守卫会逼人绕过它。
function testScannerIgnoresCommentsAndWrappers() {
  const good = [
    '// Wry 0.55.1 macOS 未实现原生 confirm() 面板，统一走 uiConfirm()',
    '/* 历史：这里曾经是 alert("x")，已废弃 */',
    'const ok = await uiConfirm("执行吗？", { title: "确认" });',
    'function uiConfirm(message, opts) { return new Promise(() => {}); }',
    'logger.confirm(payload);',
    'const url = "http://example.com//confirm(";',
  ].join('\n');
  assert.deepEqual(scanNativeDialogCalls(good), [], '合法写法与注释不得误报');
}

function main() {
  testAppJsHasNoNativeDialogs();
  testScannerCatchesViolations();
  testScannerIgnoresCommentsAndWrappers();
  console.log('native-dialogs.test.js: ok');
}

try {
  main();
} catch (error) {
  console.error(error);
  process.exitCode = 1;
}
