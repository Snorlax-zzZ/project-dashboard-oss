// 决策 6 · P1 回归：顶栏同步状态文案纯函数 computeSyncStatusView 的 notice 优先级。
// 核心命题——「未配置 / 推送失败」这类非成功结局，绝不被旧的成功时间戳"复活"成假成功；
// 且该状态持久在 state.syncStatus.notice，后续任意 refreshAll 反复渲染都保持不变。
//
// 运行：node dashboard-tauri/tests/sync-status.test.js （期望末尾打印 sync-status.test.js: ok）

const assert = require('node:assert/strict');
const path = require('node:path');

const ui = require(path.join(__dirname, '..', 'dist', 'app.js'));

// P1 核心：成功过（last 有值）之后又推送失败（notice 有值）→ 必须显示失败文案，绝不显示旧时间戳。
function testNoticeTakesPriorityOverLast() {
  const last = new Date(Date.now() - 60_000); // 1 分钟前曾成功同步
  const view = ui.computeSyncStatusView(
    { last, notice: '已本地保存·推送失败·下次重试' },
    Date.now(),
  );
  assert.equal(
    view.text,
    '已本地保存·推送失败·下次重试',
    'notice 必须优先于 last —— 失败态不得被旧成功时间复活（P1 根因）',
  );
  assert.doesNotMatch(view.text, /最后同步/, '有 notice 时绝不显示「最后同步 X 分钟前」');
  assert.equal(view.stale, true);
}

// 模拟"成功 → 未配置 → 反复 refreshAll"：每次渲染都应稳定停在"未配置"，不被 last 复活。
function testUnconfiguredNoticeSurvivesRepeatedRefresh() {
  const state = {
    last: new Date(Date.now() - 300_000), // 5 分钟前成功过
    notice: '未配置存储仓，未触发同步',
  };
  for (let i = 0; i < 3; i++) {
    const view = ui.computeSyncStatusView(state, Date.now());
    assert.equal(view.text, '未配置存储仓，未触发同步', `第 ${i + 1} 次刷新：未配置文案不得被 last 复活`);
    assert.equal(view.stale, true);
  }
}

// notice=null（真出机成功清了 notice）+ 有 last → 显示"最后同步 X 分钟前"。
function testSuccessShowsTimestampWhenNoticeCleared() {
  const view = ui.computeSyncStatusView(
    { last: new Date(Date.now() - 120_000), notice: null },
    Date.now(),
  );
  assert.match(view.text, /^上次跨机同步成功：2 分钟前$/, 'notice=null + 有 last → 明确显示跨机同步时间');
  assert.equal(view.stale, false, '正常成功态非 stale');
}

// 无记录不等于从未同步，旧版本和重启都可能没有前端内存状态。
function testNeverSyncedWhenNoLastNoNotice() {
  const view = ui.computeSyncStatusView({ last: null, notice: null }, Date.now());
  assert.match(view.text, /暂无同步记录/);
  assert.doesNotMatch(view.text, /尚未同步过|首次采集/);
  assert.equal(view.stale, false);
}

// 超过 6h（>360min）无 notice → stale + 超期提示（保留原有语义不回归）。
function testStaleWhenOverSixHours() {
  const view = ui.computeSyncStatusView(
    { last: new Date(Date.now() - 400 * 60_000), notice: null },
    Date.now(),
  );
  assert.match(view.text, /数据偏旧，建议手动同步/);
  assert.doesNotMatch(view.text, /周期触发上限/, '决策 5：去掉指向已删 6h 自动周期的措辞');
  assert.equal(view.stale, true);
}

function main() {
  testNoticeTakesPriorityOverLast();
  testUnconfiguredNoticeSurvivesRepeatedRefresh();
  testSuccessShowsTimestampWhenNoticeCleared();
  testNeverSyncedWhenNoLastNoNotice();
  testStaleWhenOverSixHours();
  console.log('sync-status.test.js: ok');
}

try {
  main();
} catch (error) {
  console.error(error);
  process.exitCode = 1;
}
