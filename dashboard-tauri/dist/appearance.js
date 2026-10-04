// 本机外观偏好：在首次绘制前确定主题，不接触业务状态或 IPC。
(function () {
  'use strict';
  const storageKey = 'project-dashboard.appearance';
  let mode = 'light';
  try {
    if (window.matchMedia('(prefers-color-scheme: dark)').matches) mode = 'dark';
  } catch (_error) { /* 不支持系统偏好查询时使用浅色。 */ }
  try {
    const saved = window.localStorage.getItem(storageKey);
    if (saved === 'light' || saved === 'dark') mode = saved;
  } catch (_error) { /* WebView 存储不可用不应阻塞启动。 */ }

  function applyTheme() {
    const root = document.documentElement;
    root.dataset.mode = mode;
    root.dataset.theme = mode === 'dark' ? '19' : '16';
    root.dataset.system = mode === 'dark' ? 'pill' : 'tactile';
    root.dataset.iconFrame = 'plain';
  }
  applyTheme();

  function bindAppearance() {
    const button = document.getElementById('btn-theme-toggle');
    const label = document.getElementById('appearance-label');
    if (!button) return;
    function updateButton() {
      button.setAttribute('aria-pressed', String(mode === 'dark'));
      button.title = mode === 'dark' ? '切换到浅色' : '切换到深色';
      if (label) label.textContent = mode === 'dark' ? '深色' : '浅色';
    }
    button.addEventListener('click', () => {
      mode = mode === 'dark' ? 'light' : 'dark';
      applyTheme();
      updateButton();
      try { window.localStorage.setItem(storageKey, mode); } catch (_error) { /* 本次选择仍立即生效。 */ }
    });
    updateButton();
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindAppearance, { once: true });
  else bindAppearance();
})();
