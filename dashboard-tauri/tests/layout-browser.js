/* 在真实浏览器执行的布局回归；使用生产 DOM/CSS/JS，不复制组件实现。 */
const frame = document.getElementById('app');
const results = [];
const waitFor = async predicate => {
  const start = performance.now();
  while (!predicate()) {
    if (performance.now() - start > 5000) throw new Error('等待界面就绪超时');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
};
const paint = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
const rect = el => el.getBoundingClientRect();
function record(name, ok, detail) { results.push({name, ok: !!ok, detail}); }
async function run() {
  results.length = 0;
  document.getElementById('run').disabled = true;
  document.getElementById('status').textContent = '运行中…';
  try {
    const d = frame.contentDocument, w = frame.contentWindow;
    const $ = selector => d.querySelector(selector);
    const change = (selector, value, event = 'change') => { $(selector).value = value; $(selector).dispatchEvent(new w.Event(event, {bubbles:true})); };
    const ready = () => $('#session-table tbody .btn-favorite') && $('#app-version').textContent;
    await waitFor(ready);
    for (const [width, height] of [[960,600],[1200,800],[1600,1000],[2560,1410]]) {
      frame.width = width; frame.height = height; await paint();
      for (const mode of ['light','dark']) {
        if (d.documentElement.dataset.mode !== mode) $('#btn-theme-toggle').click();
        for (const source of ['claude-cli','codex','pi','grok']) {
          $(`[data-session-source="${source}"]`).click();
          await waitFor(() => $(`[data-session-source="${source}"]`).getAttribute('aria-selected') === 'true' && $('#session-table tbody .btn-favorite'));
          for (const size of [10,20,50]) {
            change('#session-page-size', String(size));
            await waitFor(() => $('#session-table tbody').rows.length === size); await paint();
            const label = `${width}×${height}/${mode}/${source}/${size}`;
            const pager = rect($('#session-pagination')), tableScroll = rect($('#page-sessions .table-scroll')), main = $('#main');
            record(label + '/分页可见且与滚动列表不重叠', pager.bottom <= height + 1 && pager.top >= tableScroll.bottom - 1 && pager.left >= rect(main).left, {pager: pager.toJSON(), table: tableScroll.toJSON()});
            record(label + '/主区无横向溢出', main.scrollWidth <= main.clientWidth + 1, {scroll:main.scrollWidth,client:main.clientWidth});
            const row = $('#session-table tbody tr'), icon = row.querySelector('.favorite-icon');
            const textRange = d.createRange(); textRange.selectNodeContents(row.cells[1]);
            const text = textRange.getBoundingClientRect(), glyph = icon && rect(icon);
            record(label + '/星星与文字垂直居中', glyph && Math.abs(glyph.y + glyph.height/2 - text.y - text.height/2) <= 2, {icon:glyph?.toJSON(),text:text.toJSON()});
            const hiddenIcon = rect(row.querySelector('.session-visibility-icon'));
            const marks = [...row.querySelectorAll('.session-marks button')].map(rect);
            record(label + '/隐藏图标与星标同排且点击范围足够', marks.length === 2 && marks.every(r => r.width >= 44 && r.height >= 44) && Math.abs(marks[0].y-marks[1].y)<1 && Math.abs(hiddenIcon.y+hiddenIcon.height/2-text.y-text.height/2)<=2);
            record(label + '/显示范围与搜索框同高', Math.abs(rect($('#session-visibility')).height-rect($('#session-query')).height)<1);

            const baseline = pager.bottom;
            $('#page-sessions .table-scroll').scrollTop = 500; await paint();
            record(label + '/滚动时分页不漂移', Math.abs(rect($('#session-pagination')).bottom - baseline) < 1);
            $('#page-sessions .table-scroll').scrollTop = 0;
          }
          // 分页与收藏仍通过生产事件调用模拟 IPC，不触发抽屉。
          $('#session-pagination-pages button:last-child').click();
          await waitFor(() => $('#session-pagination-info').textContent.includes('条'));
          await paint();
          const lastRowCount = $('#session-table tbody').rows.length;
          record(`${width}/${mode}/${source}/末页行数不超过页大小`, lastRowCount > 0 && lastRowCount <= 50);
          $('#session-pagination-pages button:first-child').click(); await paint();
          const favorite = $('#session-table tbody .btn-favorite'), sid = favorite.dataset.sessionId, old = favorite.getAttribute('aria-pressed');
          favorite.focus();
          favorite.click();
          await waitFor(() => $(`[data-session-id="${sid}"]`)?.getAttribute('aria-pressed') !== old);
          record(`${width}/${mode}/${source}/收藏切换不打开抽屉`, !$('#drawer').classList.contains('open'));
          record(`${width}/${mode}/${source}/收藏重排后焦点仍在原会话星标`, d.activeElement === $(`.btn-favorite[data-session-id="${sid}"]`));
          const hide = $('#session-table tbody .btn-session-visibility'), hiddenSid = hide.dataset.visibilityId;
          const favoriteState = $(`.btn-favorite[data-session-id="${hiddenSid}"]`).getAttribute('aria-pressed');
          hide.focus();
          hide.click();
          await waitFor(() => $('#confirm-backdrop').classList.contains('open')); await paint();
          const confirmation = rect($('#confirm-backdrop .modal'));
          record(`${width}/${mode}/${source}/隐藏确认默认取消且未移除行`, d.activeElement === $('#confirm-cancel') && !!$(`[data-visibility-id="${hiddenSid}"]`) && $('#confirm-message').textContent.includes(hiddenSid));
          record(`${width}/${mode}/${source}/隐藏确认在视口内`, confirmation.top >= 0 && confirmation.bottom <= height && confirmation.right <= width);
          $('#confirm-message').textContent += '\n' + 'A'.repeat(160);
          await paint();
          record(`${width}/${mode}/${source}/长会话标题不撑出确认框`, $('#confirm-backdrop .modal').scrollWidth <= $('#confirm-backdrop .modal').clientWidth + 1);
          $('#confirm-cancel').click(); await paint();
          record(`${width}/${mode}/${source}/取消隐藏保留行与收藏`, !!$(`[data-visibility-id="${hiddenSid}"]`) && $(`.btn-favorite[data-session-id="${hiddenSid}"]`).getAttribute('aria-pressed') === favoriteState);
          $(`[data-visibility-id="${hiddenSid}"]`).click();
          await waitFor(() => $('#confirm-backdrop').classList.contains('open'));
          $('#confirm-ok').click();
          await waitFor(() => !$(`[data-visibility-id="${hiddenSid}"]`));
          record(`${width}/${mode}/${source}/确认隐藏后焦点回到稳定控件`, d.activeElement === $('#session-visibility'), {id:d.activeElement?.id,tag:d.activeElement?.tagName});
          change('#session-visibility', 'hidden');
          await waitFor(() => $(`[data-visibility-id="${hiddenSid}"]`)?.getAttribute('aria-label') === '恢复显示');
          record(`${width}/${mode}/${source}/隐藏范围只展示隐藏项`, $('#session-table tbody').rows.length === 1 && $('#session-pagination-info').textContent.includes('共 1 条'));
          $(`[data-visibility-id="${hiddenSid}"]`).focus();
          $(`[data-visibility-id="${hiddenSid}"]`).click();
          await waitFor(() => $('#session-pagination-info').textContent === '共 0 条');
          record(`${width}/${mode}/${source}/恢复后焦点回显示范围`, d.activeElement === $('#session-visibility'));
          change('#session-visibility', 'visible');
          await waitFor(() => $(`.btn-favorite[data-session-id="${hiddenSid}"]`));
          record(`${width}/${mode}/${source}/恢复保留收藏且不打开详情`, $(`.btn-favorite[data-session-id="${hiddenSid}"]`).getAttribute('aria-pressed') === favoriteState && !$('#drawer').classList.contains('open'));

        }
        await checkTableAccess(d, w, width, mode, 'sessions');
        // 已有功能页，以及详情/表单在最低支持尺寸下的可达性。
        $('#session-table tbody tr td:nth-child(5)').click();
        await waitFor(() => $('#drawer').classList.contains('open')); await new Promise(r => setTimeout(r, 260));
        $('#session-table tbody .btn-session-visibility').click();
        await waitFor(() => $('#confirm-backdrop').classList.contains('open'));
        d.dispatchEvent(new w.KeyboardEvent('keydown', {key:'Escape', bubbles:true}));
        await paint();
        record(`${width}/${mode}/取消隐藏只关闭顶层确认`, !$('#confirm-backdrop').classList.contains('open') && $('#drawer').classList.contains('open'));
        $('#drawer-title').textContent = '验证超长会话标题与路径 / '.repeat(24); await paint();
        const close = rect($('#drawer-close')), title = rect($('#drawer-title'));
        record(`${width}/${mode}/抽屉关闭按钮不被长标题挤出`, close.right <= width && close.left >= title.right - 1 && close.width >= 40);
        record(`${width}/${mode}/打开详情后分页可见`, rect($('#session-pagination')).right <= width && $('#main').scrollWidth <= $('#main').clientWidth + 1);
        const handle = $('#drawer-resizer'), startX = rect(handle).x;
        handle.dispatchEvent(new w.MouseEvent('mousedown', {bubbles:true,clientX:startX}));
        d.dispatchEvent(new w.MouseEvent('mousemove', {bubbles:true,clientX:startX - 1000}));
        d.dispatchEvent(new w.MouseEvent('mouseup', {bubbles:true})); await paint();
        record(`${width}/${mode}/抽屉拖到最大仍不挤坏分页`, $('#main').scrollWidth <= $('#main').clientWidth + 1 && rect($('#session-pagination')).bottom <= height);
        d.documentElement.style.removeProperty('--drawer-width');
        $('#drawer-close').click(); await new Promise(r => setTimeout(r, 260));
        for (const tab of ['fleet','projects','repowiki','settings']) {
          $(`[data-tab="${tab}"]`).click(); await paint();
          if (['fleet','projects'].includes(tab)) await checkTableAccess(d, w, width, mode, tab);
          if (tab === 'fleet') {
            $('#fleet-table tbody tr').click(); await paint();
            record(`${width}/${mode}/Fleet 不展示研发占位文案`, !/M4|M3|list_project_events|backlog/.test($('#page-fleet').textContent + $('#drawer').textContent));
            $('#drawer-close').click();
          }
          if (tab === 'repowiki') {
            await waitFor(() => $('.repowiki-nav-item'));
            $('.repowiki-nav-item').click();
            await waitFor(() => $('#repowiki-article').innerText.includes('项目总览'));
            $('#main').scrollTop = $('#main').scrollHeight; await paint();
            $('#repowiki-article .wiki-page-link').click();
            await waitFor(() => $('#repowiki-article h1')?.textContent === '系统架构'); await paint();
            record(`${width}/${mode}/Wiki 内链切页后标题可见`, rect($('#repowiki-article h1')).top >= rect($('#main')).top && rect($('#repowiki-article h1')).bottom <= height);
            record(`${width}/${mode}/代码知识下拉与按钮同高`, Math.abs(rect($('#repowiki-project-select')).height - rect($('#btn-repowiki-refresh')).height) < 1);
          }
          const main = $('#main');
          record(`${width}/${mode}/${tab}/页面不横向溢出`, main.scrollWidth <= main.clientWidth + 1, {scroll:main.scrollWidth,client:main.clientWidth});
          if (tab === 'projects') {
            const rowActions = [...$('#project-table tbody tr').querySelectorAll('[data-action]')].map(rect);
            const tableViewport = rect($('#page-projects .table-scroll'));
            record(`${width}/${mode}/项目操作并排且可见`, rowActions.length === 2 && Math.abs(rowActions[0].y - rowActions[1].y) < 1 && rowActions[1].right <= tableViewport.right && rowActions[0].left >= tableViewport.left, rowActions.map(r=>r.toJSON()));
            $('#project-query').value = 'definitely-no-match';
            $('#project-query').dispatchEvent(new w.Event('input',{bubbles:true}));
            await waitFor(() => !$('#project-table [data-action=edit]'));
            record(`${width}/${mode}/项目搜索空态不是无项目`, $('#project-table').textContent.includes('无匹配项目'));
            $('#project-query').value = ''; $('#project-query').dispatchEvent(new w.Event('input',{bubbles:true}));
            await waitFor(() => $('#project-table [data-action=edit]'));
            $('#btn-add-project').focus(); $('#btn-add-project').click(); await paint();
            const modal = $('#project-modal-backdrop .modal'), actions = modal.querySelector('.actions');
            record(`${width}/${mode}/项目弹窗在视口内`, rect(modal).bottom <= height && rect(modal).top >= 0);
            record(`${width}/${mode}/表单控件不收缩`, [...modal.querySelectorAll('.input')].every(el=>rect(el).height >= 40));
            record(`${width}/${mode}/项目弹窗初始焦点`, d.activeElement === $('#f-name'));
            $('#f-name').focus();
            d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}));
            record(`${width}/${mode}/项目弹窗反向焦点圈`, d.activeElement === modal.querySelector('[type=submit]'));
            modal.querySelector('[type=submit]').focus();
            d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Tab',bubbles:true,cancelable:true}));
            record(`${width}/${mode}/项目弹窗正向焦点圈`, d.activeElement === $('#f-name'));
            const confirmation = w.uiConfirm('测试关联确认');
            d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true})); await confirmation;
            record(`${width}/${mode}/Escape 只关顶层确认`, $('#project-modal-backdrop').classList.contains('open') && !$('#confirm-backdrop').classList.contains('open'));
            modal.scrollTop = modal.scrollHeight; await paint();
            record(`${width}/${mode}/弹窗操作可达`, rect(actions).bottom <= rect(modal).bottom + 1);
            $('#btn-cancel-project').click();
            record(`${width}/${mode}/关闭弹窗恢复入口焦点`, d.activeElement === $('#btn-add-project'));
            $('#project-table [data-action=edit]').focus(); $('#project-table [data-action=edit]').click(); await paint();
            record(`${width}/${mode}/重新打开从顶部开始`, modal.scrollTop === 0 && d.activeElement === $('#f-name'));
            d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));
            record(`${width}/${mode}/Escape 关闭编辑并恢复焦点`, !$('#project-modal-backdrop').classList.contains('open') && d.activeElement === $('#project-table [data-action=edit]'));
            if ($('#project-modal-backdrop').classList.contains('open')) $('#btn-cancel-project').click();
          }
        }
        $('[data-tab="sessions"]').click(); await paint();
      }
    }
    await checkAsyncActions(frame);
    // 隐藏 50 行页大小下末页的全部两项，验证后端回退与实际分页位置。
    frame.width = 960; frame.height = 600;
    frame.src = '/app.html';
    await new Promise(resolve => frame.addEventListener('load', resolve, {once:true}));
    {
      const d = frame.contentDocument, w = frame.contentWindow;
      const $ = selector => d.querySelector(selector);
      await waitFor(() => $('#session-table tbody .btn-favorite'));
      $('[data-session-source="grok"]').click();
      await waitFor(() => $('#session-pagination-info').textContent.includes('共 52 条'));
      $('#session-page-size').value = '50';
      $('#session-page-size').dispatchEvent(new w.Event('change',{bubbles:true}));
      await waitFor(() => $('#session-table tbody').rows.length === 50);
      $('#session-pagination-pages button:last-child').click();
      await waitFor(() => $('#session-table tbody').rows.length === 2);
      for (const total of [51,50]) {
        $('#session-table tbody .btn-session-visibility').click();
        await waitFor(() => $('#confirm-backdrop').classList.contains('open'));
        $('#confirm-ok').click();
        await waitFor(() => $('#session-pagination-info').textContent.includes(`共 ${total} 条`));
      }
      await paint();
      record('隐藏末页最后一项/回退有效页且补齐50行', $('#session-pagination-info').textContent === '第 1-50 条 / 共 50 条' && $('#session-table tbody').rows.length === 50);
      const pager=rect($('#session-pagination')), scroll=rect($('#page-sessions .table-scroll'));
      record('隐藏末页最后一项/分页位置稳定',pager.bottom<=600 && pager.top>=scroll.bottom-1);
    }
    // 空态、错误态保留分页占位；无数据时也能切换来源和恢复。
    for (const scenario of ['empty','error']) {
      for (const mode of ['light','dark']) {
        frame.width = 960; frame.height = 600;
        frame.src = '/app.html?scenario=' + scenario;
        await new Promise(resolve => frame.addEventListener('load', resolve, {once:true}));
        const doc = frame.contentDocument;
        await waitFor(() => doc.querySelector('#session-pagination-info').textContent === '共 0 条');
        if (doc.documentElement.dataset.mode !== mode) doc.querySelector('#btn-theme-toggle').click();
        await paint();
        const page = rect(doc.querySelector('#session-pagination')), scroll = rect(doc.querySelector('#page-sessions .table-scroll'));
        record(`${scenario}/${mode}/空错态分页稳定`, page.bottom <= 600 && page.top >= scroll.bottom);
      }
    }
  } catch (error) { record('执行异常', false, error.stack); }
  document.getElementById('run').disabled = false;
  document.getElementById('status').textContent = `完成：${results.length} 项，${results.filter(r=>!r.ok).length} 项失败`;
  document.getElementById('results').textContent = JSON.stringify(results.filter(r=>!r.ok), null, 2);
  await fetch('/report', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({date:new Date().toISOString(),userAgent:navigator.userAgent,results})});
}
document.getElementById('run').addEventListener('click', run);

// 宽表格允许溢出，但必须能不依赖触控板手势访问所有列。
async function checkTableAccess(d, w, width, mode, tab) {
  const scroll = d.querySelector(`#page-${tab} .table-scroll`);
  const tools = scroll.previousElementSibling;
  const max = scroll.scrollWidth - scroll.clientWidth;
  const right = tools?.querySelector('[data-scroll=right]');
  const label = `${width}/${mode}/${tab}`;
  record(label + '/宽表有显式滚动入口', max <= 1 || (right && !tools.hidden));
  if (max > 1 && right) {
    for (let i=0; i<20 && scroll.scrollLeft < max-1; i++) right.click();
    await paint();
    record(label + '/可访问最右列', scroll.scrollLeft >= max-1);
    scroll.focus();
    scroll.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Home',bubbles:true,cancelable:true})); await paint();
    record(label + '/键盘回最左列', scroll.scrollLeft === 0);
    scroll.dispatchEvent(new w.KeyboardEvent('keydown',{key:'End',bubbles:true,cancelable:true})); await paint();
    record(label + '/键盘到最右列', scroll.scrollLeft >= max-1);
    scroll.scrollLeft = 0;
  }
}

// 保留真实渲染和事件，只延迟合成 IPC，覆盖请求在途的焦点与重复操作。
async function checkAsyncActions(frame) {
  const d = frame.contentDocument, w = frame.contentWindow;
  const $ = selector => d.querySelector(selector);
  const base = w.__TAURI__.core.invoke;
  let release, writes = 0;
  w.__TAURI__.core.invoke = (cmd,args) => {
    if (cmd !== 'toggle_favorite') return base(cmd,args);
    writes++;
    return new Promise(resolve => { release = () => resolve(base(cmd,args)); });
  };
  try {
    const star = $('#session-table .btn-favorite'), sid = star.dataset.sessionId;
    star.focus(); star.click();
    const pending = $(`.btn-favorite[data-session-id="${sid}"]`);
    pending.click();
    record('慢响应/收藏在途保留焦点且重复点击仅写一次', d.activeElement === pending && pending.getAttribute('aria-disabled') === 'true' && writes === 1);
    $('#session-query').focus(); release();
    await waitFor(() => $(`.btn-favorite[data-session-id="${sid}"]`)?.getAttribute('aria-disabled') === 'false');
    record('慢响应/完成后不抢走搜索框焦点', d.activeElement === $('#session-query'));
    const next = $('#session-table .btn-favorite');
    next.focus(); next.click();
    const source = $('[data-session-source="pi"]');
    source.focus(); source.click(); release(); await paint();
    record('慢响应/切来源后不抢走新来源焦点', d.activeElement === source);
  } finally { w.__TAURI__.core.invoke = base; }
  w.__TAURI__.core.invoke = (cmd,args) => cmd === 'set_session_hidden'
    ? Promise.reject(new Error('测试磁盘写失败')) : base(cmd,args);
  try {
    const hide = $('#session-table .btn-session-visibility'), sid = hide.dataset.visibilityId;
    hide.focus(); hide.click(); $('#confirm-ok').click();
    await waitFor(() => $('#session-notice').textContent.includes('保存失败'));
    record('写入失败/保留原行与键盘焦点', d.activeElement === $(`[data-visibility-id="${sid}"]`));
  } finally { w.__TAURI__.core.invoke = base; }
  $('[data-tab="projects"]').click(); await paint();
  const edit = $('#project-table [data-action=edit]'); edit.focus(); edit.click();
  writes = 0;
  w.__TAURI__.core.invoke = (cmd,args) => {
    if (cmd !== 'edit_project') return base(cmd,args);
    writes++; return new Promise(resolve => { release = () => resolve(base(cmd,args)); });
  };
  try {
    $('#project-form [type=submit]').focus();
    $('#project-form').dispatchEvent(new w.Event('submit',{bubbles:true,cancelable:true}));
    $('#project-form').dispatchEvent(new w.Event('submit',{bubbles:true,cancelable:true}));
    d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));
    record('慢响应/项目保存不重复提交且不被 Escape 隐藏', writes === 1 && $('#project-modal-backdrop').classList.contains('open'));
    d.activeElement.blur();
    d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}));
    record('慢响应/保存失焦后反向 Tab 回最后可用字段', d.activeElement === $('#f-mac-path'));
    release();
    await waitFor(() => !$('#project-modal-backdrop').classList.contains('open')); await paint();
    record('慢响应/保存完成后恢复编辑入口', d.activeElement?.dataset.projectId === edit.dataset.projectId && d.activeElement?.dataset.action === 'edit');
  } finally { w.__TAURI__.core.invoke = base; }
}
