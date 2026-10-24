/* 中英文切换：仅对合成 IPC 与测试窗口操作，不访问本机真实数据。 */
const frame = document.getElementById('app');
const results = [];
const paint = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
const record = (name, ok, detail) => results.push({name,ok:!!ok,detail});
async function ready(predicate) {
  const start=performance.now();
  while(!predicate()) {
    if(performance.now()-start>5000) throw new Error('等待语言验收页面超时');
    await new Promise(r=>setTimeout(r,20));
  }
}
async function run() {
  results.length=0; document.getElementById('run').disabled=true;
  document.getElementById('status').textContent='运行中：中英文、五页、模态、分页与深浅布局';
  const saved = localStorage.getItem('project-dashboard.language');
  try {
    await ready(()=>frame.contentDocument.querySelector('#session-table tbody .btn-favorite'));
    let d=frame.contentDocument,w=frame.contentWindow;
    const $=s=>d.querySelector(s),rect=el=>el.getBoundingClientRect();
    const change=async language=>{$('#ui-language').value=language;$('#ui-language').dispatchEvent(new w.Event('change',{bubbles:true}));await paint();};
    await change('zh-CN');
    record('首次中文入口', $('[data-tab="sessions"]').textContent.trim()==='会话看板');
    $('#session-page-size').value='50';$('#session-page-size').dispatchEvent(new w.Event('change',{bubbles:true}));
    await ready(()=>$('#session-table tbody').rows.length===50);
    [...d.querySelectorAll('#session-pagination-pages button')].find(b=>b.textContent==='2').click();
    await ready(()=>$('#session-pagination-info').textContent.includes('51-100'));
    $('#session-table tbody tr').click();await ready(()=>$('#insp-copy-resume'));
    $('#in-state-root').value='~/中文目录/会话看板';
    const conversation=$('.chat-stream').textContent,detailTitle=$('.inspector-title').textContent;
    const calls=w.layoutBackend.calls.length,machine=$('#sidebar-machine-id').textContent;
    await change('en');
    record('切换不产生任何 IPC',w.layoutBackend.calls.length===calls);
    record('分页状态与每页50保留',$('#session-pagination-info').textContent==='51–100 of 143'&&$('#session-page-size').value==='50');
    record('详情内容和原文标题保留',$('.chat-stream').textContent===conversation&&$('.inspector-title').textContent===detailTitle);
    record('表单输入原文不变',$('#in-state-root').value==='~/中文目录/会话看板');
    record('真实机器身份不被占位覆盖',$('#sidebar-machine-id').textContent===machine);
    record('侧栏五个入口英文',[...d.querySelectorAll('[data-tab]')].map(b=>b.textContent.trim()).join('|')==='Session Board|Fleet Board|Projects|Code Wiki|Settings');
    record('详情动作与文件组英文',$('#insp-copy-resume').textContent.includes('Copy resume command')&&$('.files-drawer summary').textContent.includes('Changed files'));
    record('表格可访问名称英文',$('#session-table-viewport').getAttribute('aria-label').startsWith('Session list'));
    record('同步状态英文',$('#sync-status').textContent==='Cross-machine sync: no sync history');
    const beforeHide=w.layoutBackend.calls.filter(c=>c.cmd==='set_session_hidden').length;
    $('#session-table .btn-session-visibility').click();await paint();
    record('隐藏二次确认英文',$('#confirm-title').textContent==='Hide this session?'&&$('#confirm-ok').textContent==='Hide session'&&$('#confirm-message').textContent.includes('does not delete or archive'));
    $('#confirm-cancel').click();await paint();
    record('取消隐藏不写数据',w.layoutBackend.calls.filter(c=>c.cmd==='set_session_hidden').length===beforeHide);
    $('#session-source-manage').click();await paint();
    record('来源管理标题与手柄英文',$('#session-source-manager-title').textContent==='Manage source tabs'&&$('[data-source-drag="codex"]').getAttribute('aria-label')==='Drag to reorder Codex');
    record('来源选择数量英文',$('#session-source-selection-status').textContent==='9 of 9 sources selected');
    $('#session-source-cancel').click();
    for(const id of ['claude-cli','codex','pi','grok','opencode','antigravity','dsh','zcode','gemini-cli']) {
      $(`[data-session-source="${id}"]`).click();await paint();
      await ready(()=>$('#session-table .btn-session-visibility'));
      record(id+'/标记动作英文',$('#session-table .btn-session-visibility').getAttribute('aria-label')==='Hide');
      if(!['claude-cli','codex','pi','grok'].includes(id))record(id+'/来源说明英文',!/[\u3400-\u9fff]/.test($('#session-source-hint').textContent));
      $('#session-table tbody tr').click();await ready(()=>$('#insp-copy-resume'));
      record(id+'/详情按钮英文',$('#insp-copy-resume').textContent.includes(['dsh','zcode','gemini-cli'].includes(id)?'Copy session ID':'Copy resume command'));
      if(id==='antigravity')record('摘要限制英文',$('.session-detail-notice').textContent.includes('summaries only'));
    }
    $('[data-tab="fleet"]').click();await ready(()=>$('#fleet-table tbody tr'));
    $('#fleet-table tbody tr').click();await paint();
    record('Fleet详情英文',$('#fleet-inspector').textContent.includes('Machine details')&&$('#fleet-inspector').textContent.includes('Last 5 commits'));
    $('[data-tab="projects"]').click();$('#btn-add-project').click();await paint();
    record('新增项目表单英文',$('#modal-title').textContent==='Add project'&&d.querySelector('label[for="f-name"]').textContent==='Project name');
    $('#f-name').value='会话看板 $& {0}';$('#f-git').value='https://example.com/测试.git';
    await change('zh-CN');await change('en');
    record('表单切换不改变用户值',$('#f-name').value==='会话看板 $& {0}'&&$('#f-git').value==='https://example.com/测试.git');
    $('#btn-cancel-project').click();
    $('#project-table tbody [data-action="delete"]').click();await paint();
    record('删除确认英文',$('#confirm-title').textContent==='Delete project'&&$('#confirm-cancel').textContent==='Cancel'&&$('#confirm-message').textContent.includes('registry entry'));
    $('#confirm-cancel').click();
    $('[data-tab="repowiki"]').click();await ready(()=>$('.repowiki-nav-item[data-page-id="home"]'));
    $('.repowiki-nav-item[data-page-id="home"]').click();await ready(()=>$('.repowiki-markdown'));
    record('Wiki工具栏与状态英文',$('#repowiki-wiki-state').textContent.startsWith('Status:')&&$('#repowiki-find-query').placeholder==='Find in current document…');
    const wiki=$('.repowiki-markdown').textContent;
    $('#repowiki-find-query').value='project-dashboard';$('#repowiki-find-query').dispatchEvent(new w.Event('input',{bubbles:true}));await paint();
    const find=$('#repowiki-find-count').textContent;
    const wikiCalls=w.layoutBackend.calls.length;
    await change('zh-CN');await change('en');
    record('Wiki正文、查找与状态保留',$('.repowiki-markdown').textContent===wiki&&$('#repowiki-find-count').textContent===find&&$('#repowiki-find-query').value==='project-dashboard');
    record('Wiki切换不请求后端',w.layoutBackend.calls.length===wikiCalls);
    $('#repowiki-graph-query').value='list';$('#repowiki-graph-search-form').dispatchEvent(new w.Event('submit',{bubbles:true,cancelable:true}));
    await ready(()=>$('.repowiki-node-card'));
    record('图谱动作英文',$('#repowiki-graph-results').textContent.includes('Callers')&&$('#repowiki-graph-results').textContent.includes('Callees'));
    // 已产生的失败提示也要跟随语言变化，原始诊断文本保持原样。
    $('[data-tab="sessions"]').click();await change('zh-CN');
    const originalInvoke=w.layoutBackend.invoke.bind(w.layoutBackend);
    w.layoutBackend.invoke=(cmd,args)=>cmd==='toggle_favorite'?Promise.reject(new Error('合成故障原文')):originalInvoke(cmd,args);
    $('#session-table .btn-favorite').click();
    await ready(()=>$('#session-notice').textContent.includes('收藏保存失败'));
    const errorCalls=w.layoutBackend.calls.length;
    await change('en');
    record('已发生的收藏失败同步变英文',$('#session-notice').textContent.includes('Could not save favorite:')&&$('#session-notice').textContent.includes('合成故障原文'));
    record('失败提示换语言不重试写入',w.layoutBackend.calls.length===errorCalls);
    w.layoutBackend.invoke=originalInvoke;
    w.layoutBackend.setScenario('empty');await w.fetchSessions(1,50);
    record('空会话状态英文',$('#session-table tbody').textContent.includes('No visible sessions')&&$('#session-pagination-info').textContent==='0 sessions');
    w.layoutBackend.setScenario('error');await w.fetchSessions(1,50);
    record('读取失败有英文说明和原始诊断',$('#session-table tbody').textContent.includes('Could not load')&&$('#session-table tbody').textContent.includes('演示：会话缓存读取失败'));
    w.layoutBackend.setScenario('push-failed');await w.manualSync();
    record('推送失败不误报成功且英文',$('#sync-status').textContent==='Saved locally · Push failed · Retry next time');
    await change('zh-CN');record('同步失败可切回中文',$('#sync-status').textContent==='已本地保存·推送失败·下次重试');await change('en');
    w.layoutBackend.setScenario('normal');await w.refreshAll();
    for(const [width,height]of [[960,600],[1200,800]]) {
      frame.width=width;frame.height=height;await paint();
      for(const mode of ['light','dark']) {
        if(d.documentElement.dataset.mode!==mode)$('#btn-theme-toggle').click();
        await paint();
        const label=`${width}×${height}/${mode}`;
        for(const page of ['sessions','fleet','projects','repowiki','settings']) {
          $(`[data-tab="${page}"]`).click();await paint();
          record(label+'/'+page+'/主区不横溢',$('#main').scrollWidth<=$('#main').clientWidth+1);
          if(page==='sessions') {
            const pager=rect($('#session-pagination')),list=rect($('#session-table-viewport'));
            record(label+'/分页固定可见',pager.bottom<=height+1&&pager.top>=list.bottom-1);
          }
          if(page==='settings')record(label+'/语言入口完整',rect($('#ui-language')).right<=width&&rect($('#ui-language')).bottom<=height);
        }
        $('#btn-manage-session-sources').click();await paint();
        const dialog=rect($('#session-source-manager')),footer=rect($('.session-source-manager-footer'));
        record(label+'/来源弹窗与底部操作完整',dialog.top>=0&&dialog.bottom<=height&&footer.bottom<=height);
        $('#session-source-cancel').click();
      }
    }
    // 重启验证必须载入实际文档，不能只读取 storage 假定首屏已翻译。
    await new Promise(resolve=>{frame.addEventListener('load',resolve,{once:true});frame.src='/app.html?i18n-restart';});
    d=frame.contentDocument;w=frame.contentWindow;
    await ready(()=>d.querySelector('#session-table .btn-favorite'));
    record('重载恢复英文并生成英文动态列表',d.documentElement.lang==='en'&&d.querySelector('[data-tab="sessions"]').textContent.trim()==='Session Board'&&d.querySelector('#session-table .btn-session-visibility').getAttribute('aria-label')==='Hide');
  } catch(e) {record('回归无异常',false,String(e));}
  if(saved===null)localStorage.removeItem('project-dashboard.language');else localStorage.setItem('project-dashboard.language',saved);
  const failures=results.filter(r=>!r.ok);
  document.getElementById('status').textContent=`完成：${results.length-failures.length}/${results.length} 通过`;
  document.getElementById('results').textContent=JSON.stringify(failures,null,2);
  await fetch('/report?suite=i18n',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({results})});
  document.getElementById('run').disabled=false;
}
document.getElementById('run').addEventListener('click',run);
