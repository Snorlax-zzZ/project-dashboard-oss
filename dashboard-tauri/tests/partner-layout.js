/* 正式前端 + 合成 IPC；九来源分页与能力差异的浏览器回归。 */
const frame = document.getElementById('app');
const results = [];
const paint = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
const record = (name, ok, detail) => results.push({name,ok:!!ok,detail});
async function ready(predicate) {
  const start=performance.now();
  while (!predicate()) {
    if(performance.now()-start>5000) throw new Error('等待界面超时');
    await new Promise(r=>setTimeout(r,20));
  }
}
async function run() {
  results.length=0; document.getElementById('run').disabled=true;
  document.getElementById('status').textContent='运行中：九来源 · 深浅主题 · 50 行分页';
  try {
    const d=frame.contentDocument,w=frame.contentWindow,$=s=>d.querySelector(s),rect=e=>e.getBoundingClientRect();
    await ready(()=>$('#session-table tbody tr'));
    const ids=['claude-cli','codex','pi','grok','opencode','antigravity','dsh','zcode','gemini-cli'];
    record('九个工具名 Tab',d.querySelectorAll('.session-source-tab').length===9);
    for (const [width,height] of [[960,600],[1200,800],[1600,1000]]) {
      frame.width=width;frame.height=height;await paint();
      for (const mode of ['light','dark']) {
        if(d.documentElement.dataset.mode!==mode)$('#btn-theme-toggle').click();
        for (const id of ids) {
          const tab=$(`[data-session-source="${id}"]`);tab.click();await paint();
          $('#session-page-size').value='50';$('#session-page-size').dispatchEvent(new w.Event('change',{bubbles:true}));
          await ready(()=>$('#session-table tbody').rows.length===50);await paint();
          const label=`${width}×${height}/${mode}/${id}`;
          const viewport=rect($('.session-source-tabs')),selected=rect(tab),pager=rect($('#session-pagination')),list=rect($('#page-sessions .table-scroll'));
          record(label+'/选中工具可见',selected.left>=viewport.left-1&&selected.right<=viewport.right+1);
          record(label+'/工具条单行',viewport.height<=64);
          record(label+'/主区无横向溢出',$('#main').scrollWidth<=$('#main').clientWidth+1);
          record(label+'/分页独立且可见',pager.bottom<=height+1&&pager.top>=list.bottom-1,{pager:pager.toJSON(),list:list.toJSON()});
          $('#page-sessions .table-scroll').scrollTop=400;await paint();
          record(label+'/滚动列表不移动分页',Math.abs(rect($('#session-pagination')).bottom-pager.bottom)<1);
          $('#session-table tbody tr').click();await ready(()=>$('#insp-copy-resume'));await paint();
          const copy=$('#insp-copy-resume').textContent;
          record(label+'/按能力显示复制动作',['dsh','zcode','gemini-cli'].includes(id)?copy.includes('复制会话 ID'):copy.includes('复制恢复命令'));
          if(id==='antigravity') record(label+'/摘要不会伪装对话',$('#session-inspector').textContent.includes('暂不支持完整对话')&&!$('#session-inspector').textContent.includes('最近对话'));
        }
        const last=$('[data-session-source="gemini-cli"]');
        last.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Home',bubbles:true}));await paint();
        record(`${width}/${mode}/Home 切回首项`,$('[data-session-source="claude-cli"]').getAttribute('aria-selected')==='true');
        $('[data-session-source="claude-cli"]').dispatchEvent(new w.KeyboardEvent('keydown',{key:'End',bubbles:true}));await paint();
        record(`${width}/${mode}/End 可达末项`,last.getAttribute('aria-selected')==='true');
      }
    }
    // SQL 来源搜索只承诺元信息；摘要来源明确限制。
    $('[data-session-source="opencode"]').click();await paint();
    record('OpenCode 搜索范围准确',$('#session-query').placeholder==='搜索标题 / cwd / 会话 ID');
    record('工具条有显式滚动入口',!!$('#session-source-previous')&&!!$('#session-source-next'));
    $('[data-session-source="zcode"]').click();await paint();
    const star=$('#session-table tbody .btn-favorite');star.click();await paint();
    record('新来源收藏 IPC 带 source',w.layoutBackend.calls.some(c=>c.cmd==='toggle_favorite'&&c.args.source==='zcode'));
  } catch(e) {record('回归执行无异常',false,String(e));}
  const failures=results.filter(r=>!r.ok);
  document.getElementById('status').textContent=`完成：${results.length-failures.length}/${results.length} 通过`;
  document.getElementById('results').textContent=JSON.stringify(failures,null,2);
  await fetch('/report?suite=partners',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({results})});
  document.getElementById('run').disabled=false;
}
document.getElementById('run').addEventListener('click',run);
