// 本机外观偏好：在首次绘制前确定主题，不接触业务状态或 IPC。
(function () {
  'use strict';
  const storageKey = 'project-dashboard.appearance';
  const paletteKey = 'project-dashboard.palette';
  const darkPaletteKey = 'project-dashboard.dark-palette';

  const PALETTES = {
    obsidian: { mode: 'dark', label: '曜石翡翠' },
    alabaster: { mode: 'light', label: '宣纸水墨' },
    nord: { mode: 'dark', label: '钛金冰蓝' },
    graphite: { mode: 'dark', label: '纯黑白灰' }
  };

  let mode = 'dark';
  try {
    if (window.matchMedia && !window.matchMedia('(prefers-color-scheme: dark)').matches) {
      mode = 'light';
    }
  } catch (_error) { /* 不支持系统偏好查询时采用当前模式。 */ }

  try {
    const saved = window.localStorage.getItem(storageKey);
    if (saved === 'light' || saved === 'dark') mode = saved;
  } catch (_error) { /* WebView 存储不可用不应阻塞启动。 */ }

  let currentPalette = mode === 'light' ? 'alabaster' : 'obsidian';
  try {
    const savedPalette = window.localStorage.getItem(paletteKey);
    if (savedPalette && PALETTES[savedPalette]) {
      currentPalette = savedPalette;
      const savedMode = window.localStorage.getItem(storageKey);
      if (!savedMode) mode = PALETTES[currentPalette].mode;
    }
  } catch (_error) {}
  let lastDarkPalette = currentPalette === 'alabaster' ? 'obsidian' : currentPalette;
  try {
    const savedDark = window.localStorage.getItem(darkPaletteKey);
    if (currentPalette === 'alabaster' && PALETTES[savedDark]?.mode === 'dark') lastDarkPalette = savedDark;
  } catch (_error) {}

  function applyTheme() {
    const root = document.documentElement;
    if (!root || !root.dataset) return;
    root.dataset.mode = mode;
    root.dataset.theme = mode === 'dark' ? '19' : '16';
    root.dataset.system = mode === 'dark' ? 'pill' : 'tactile';
    root.dataset.iconFrame = 'plain';
    root.dataset.palette = currentPalette;
  }
  applyTheme();

  function syncPalettePicker() {
    if (typeof document.querySelectorAll !== 'function') return;
    document.querySelectorAll('[data-palette-option]').forEach((btn) => {
      const p = btn.dataset.paletteOption;
      const isActive = p === currentPalette;
      btn.classList.toggle('active', isActive);
      btn.setAttribute('aria-pressed', String(isActive));
    });
  }

  function setPalette(paletteName) {
    if (!PALETTES[paletteName]) return;
    currentPalette = paletteName;
    if (paletteName !== 'alabaster') lastDarkPalette = paletteName;
    mode = PALETTES[paletteName].mode;
    applyTheme();
    try {
      window.localStorage.setItem(paletteKey, currentPalette);
      window.localStorage.setItem(storageKey, mode);
      window.localStorage.setItem(darkPaletteKey, lastDarkPalette);
    } catch (_error) {}
    updateToggleButton();
    syncPalettePicker();
  }
  window.__setPalette = setPalette;

  function updateToggleButton() {
    const text = value => window.DashboardI18n ? window.DashboardI18n.t(value) : value;
    const button = document.getElementById('btn-theme-toggle');
    const label = document.getElementById('appearance-label');
    const themeName = document.getElementById('appearance-theme-name');
    if (themeName) themeName.textContent = text(PALETTES[currentPalette].label);
    if (!button) return;
    button.setAttribute('aria-pressed', String(mode === 'dark'));
    button.title = text(mode === 'dark' ? '切换到浅色' : '切换到深色');
    button.setAttribute('aria-label', text('深色模式'));
    if (label) label.textContent = text(mode === 'dark' ? '深色' : '浅色');
  }

  function closePicker(restoreFocus = false) {
    const panel = document.getElementById('appearance-picker');
    const trigger = document.getElementById('btn-appearance-picker');
    if (!panel || !trigger || panel.hidden) return;
    panel.hidden = true;
    trigger.setAttribute('aria-expanded', 'false');
    if (restoreFocus) trigger.focus();
  }

  function positionPicker() {
    const panel = document.getElementById('appearance-picker');
    const anchor = document.getElementById('btn-theme-toggle');
    if (!panel || !anchor || panel.hidden) return;
    const rect = anchor.getBoundingClientRect();
    panel.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - panel.offsetWidth - 8))}px`;
    panel.style.top = `${Math.max(8, rect.top - panel.offsetHeight - 10)}px`;
  }

  function bindAppearance() {
    document.addEventListener('dashboard:language-changed', updateToggleButton);
    const button = document.getElementById('btn-theme-toggle');
    if (button) {
      button.addEventListener('click', () => {
        setPalette(mode === 'dark' ? 'alabaster' : lastDarkPalette);
        closePicker();
      });
      updateToggleButton();
    }

    const trigger = document.getElementById('btn-appearance-picker');
    const panel = document.getElementById('appearance-picker');
    if (trigger && panel) {
      trigger.addEventListener('click', () => {
        if (!panel.hidden) { closePicker(); return; }
        panel.hidden = false;
        trigger.setAttribute('aria-expanded', 'true');
        positionPicker();
        panel.querySelector('[aria-pressed="true"]')?.focus();
      });
      document.addEventListener('keydown', (e) => {
        if (e.key !== 'Escape' || panel.hidden) return;
        e.preventDefault();
        e.stopImmediatePropagation?.();
        closePicker(true);
      });
      document.addEventListener('focusin', (e) => {
        if (!panel.contains(e.target) && !trigger.contains(e.target)) closePicker();
      });
      window.addEventListener?.('resize', positionPicker);
    }

    // 侧栏弹层与设置页复用同一套配色逻辑。
    if (typeof document.addEventListener === 'function') {
      document.addEventListener('click', (e) => {
        if (!e || !e.target) return;
        const opt = e.target.closest ? e.target.closest('[data-palette-option]') : null;
        if (opt && opt.dataset) {
          setPalette(opt.dataset.paletteOption);
          closePicker(!!panel?.contains(opt));
        } else if (panel && trigger && !panel.contains(e.target) && !trigger.contains(e.target)) {
          closePicker();
        }
      });
    }

    syncPalettePicker();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bindAppearance, { once: true });
  } else {
    bindAppearance();
  }
})();
