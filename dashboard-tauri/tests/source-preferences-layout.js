/* 正式前端 + 合成 IPC：来源显示偏好、持久化及管理面板回归。 */
const frame = document.getElementById('app');
const results = [];
const paint = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
const record = (name, ok) => results.push({name,ok:!!ok});
const key = 'pd-session-sources-v1';
const ids = ['claude-cli','codex','pi','grok','opencode','antigravity','dsh','zcode','gemini-cli'];
async function ready(predicate) {
  const start=performance.now();
  while(!predicate()) {
    if(performance.now()-start>5000) throw new Error('等待界面超时');
    await new Promise(r=>setTimeout(r,20));
  }
}
async function run() {
  results.length=0;document.getElementById('run').disabled=true;
  document.getElementById('status').textContent='运行中：来源显示、排序、重载与模态面板';
  const saved=localStorage.getItem(key);
  let d,w,$,loadCount=0;
  const load=async()=>{
    await new Promise(resolve=>{frame.addEventListener('load',resolve,{once:true});frame.src='/app.html?preferences='+ ++loadCount;});
    d=frame.contentDocument;w=frame.contentWindow;$=s=>d.querySelector(s);
    await ready(()=>$('#session-table tbody .btn-favorite'));
  };
  const visible=()=>[...d.querySelectorAll('.session-source-tab')].filter(el=>!el.hidden).map(el=>el.dataset.sessionSource);
  const active=()=>$('.session-source-tab.active')?.dataset.sessionSource;
  const check=id=>$(`[data-source-visible="${id}"]`);
  const open=()=>$('#session-source-manage').click();
  const save=async()=>{$('#session-source-save').click();await paint();};
  const move=(id,direction)=>$(`[data-source-move="${id}"][data-direction="${direction}"]`).click();
  try {
    localStorage.removeItem(key);await load();
    record('首次默认显示九来源',visible().join()===ids.join());
    $('[data-session-source="codex"]').click();await paint();
    $('#session-query').value='来源';$('#session-query').dispatchEvent(new w.Event('input',{bubbles:true}));
    await ready(()=>w.layoutBackend.calls.some(c=>c.cmd==='list_sessions'&&c.args.query==='来源'));
    $('#session-page-size').value='50';$('#session-page-size').dispatchEvent(new w.Event('change',{bubbles:true}));await paint();
    [...d.querySelectorAll('#session-pagination-pages button')].find(button=>button.textContent==='2').click();await paint();
    $('#session-table tbody tr').click();await ready(()=>$('#insp-copy-resume'));
    const pager=$('#session-pagination').textContent,detail=$('#session-inspector').textContent;
    open();move('codex',-1);check('claude-cli').click();$('#session-source-cancel').click();await paint();
    record('取消保留原选择顺序与来源',visible().join()===ids.join()&&active()==='codex');
    open();move('codex',-1);check('claude-cli').click();
    const calls=w.layoutBackend.calls.filter(c=>c.cmd==='list_sessions').length;
    await save();
    record('保存顺序与显示选择',visible()[0]==='codex'&&!visible().includes('claude-cli'));
    record('仍可见来源不重复查询',w.layoutBackend.calls.filter(c=>c.cmd==='list_sessions').length===calls);
    record('排序保留搜索分页详情',$('#session-query').value==='来源'&&$('#session-pagination').textContent===pager&&$('#session-inspector').textContent===detail);
    const first=$('[data-session-source="codex"]');first.focus();first.dispatchEvent(new w.KeyboardEvent('keydown',{key:'End',bubbles:true}));await paint();
    record('End 遵循可见顺序',active()==='gemini-cli');
    record('键盘切换保留 Tab 焦点',d.activeElement===$('[data-session-source="gemini-cli"]'));
    $('#session-table tbody tr').click();await ready(()=>$('#insp-copy-resume'));
    open();for(const id of ids)if(check(id).checked&&!['pi','codex'].includes(id))check(id).click();
    move('pi',-1);move('pi',-1);await save();
    record('隐藏当前来源后切至第一可见项',visible().join()==='pi,codex'&&active()==='pi');
    record('隐藏当前来源关闭旧详情',!$('#insp-copy-resume'));
    await load();
    record('重载恢复选择顺序',visible().join()==='pi,codex'&&active()==='pi');
    record('启动不扫描被隐藏来源',w.layoutBackend.calls.filter(c=>c.cmd==='list_sessions').every(c=>c.args.source==='pi'));
    open();check('pi').click();check('codex').click();
    record('全不选时禁止保存',$('#session-source-save').disabled&&$('#session-source-selection-status').textContent.includes('至少'));
    $('#session-source-cancel').click();await paint();
    open();check('codex').click();await save();
    const only=$('[data-session-source="pi"]');only.focus();only.dispatchEvent(new w.KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true}));await paint();
    record('单来源键盘不会进入隐藏项',active()==='pi'&&d.activeElement===only);
    record('来源减少后滚动按钮状态更新',$('#session-source-previous').disabled&&$('#session-source-next').disabled);
    // 真实存储失败：只替换该合成测试页面的 Storage 原型，随后恢复。
    open();check('codex').click();const write=w.Storage.prototype.setItem;
    try {
      w.Storage.prototype.setItem=function(k,v){if(k===key)throw new Error('synthetic quota');return write.call(this,k,v);};
      await save();
      record('写入失败保留面板和草稿',$('#session-source-manager').open&&check('codex').checked&&$('#session-source-error').textContent.includes('保存失败'));
      record('写入失败不改变已生效配置',visible().join()==='pi');
    } finally {w.Storage.prototype.setItem=write;}
    $('#session-source-cancel').click();await paint();
    $('[data-tab="settings"]').click();await paint();$('#btn-manage-session-sources').click();
    record('设置页打开同一管理面板',$('#session-source-manager').open&&check('pi').checked&&!check('codex').checked);
    check('pi').focus();check('pi').dispatchEvent(new w.KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}));
    record('Shift Tab 可到达关闭按钮',d.activeElement===$('#session-source-close'));
    $('#session-source-close').dispatchEvent(new w.KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}));
    record('Shift Tab 在面板操作间循环',d.activeElement===$('#session-source-save'));
    $('#session-source-reset').click();$('#session-source-manager').dispatchEvent(new w.Event('cancel',{cancelable:true}));await paint();
    record('恢复默认后取消不会生效',visible().join()==='pi');
    record('关闭面板恢复入口焦点',d.activeElement===$('#btn-manage-session-sources'));
    $('[data-tab="sessions"]').click();await paint();
    for(const [width,height] of [[960,600],[1200,800],[1600,1000]]) {
      frame.width=width;frame.height=height;await paint();
      for(const mode of ['light','dark']) {
        if(d.documentElement.dataset.mode!==mode)$('#btn-theme-toggle').click();
        open();$('#session-source-reset').click();await paint();
        const label=`${width}×${height}/${mode}`,dialog=$('#session-source-manager').getBoundingClientRect(),footer=$('.session-source-manager-footer').getBoundingClientRect(),list=$('#session-source-options').getBoundingClientRect();
        record(label+'/面板不超视口',dialog.top>=0&&dialog.bottom<=height&&dialog.left>=0&&dialog.right<=width);
        record(label+'/底部操作可见',footer.bottom<=height&&footer.top>=list.bottom-1);
        record(label+'/列表可独立滚动',$('#session-source-options').scrollHeight>=list.height);
        record(label+'/管理行点击空间足够',[...d.querySelectorAll('.session-source-option label')].every(el=>el.getBoundingClientRect().height>=44));
        $('#session-source-cancel').click();await paint();
      }
    }
    open();$('#session-source-reset').click();await save();
    record('恢复默认重新显示全部来源',visible().join()===ids.join());
    // PointerEvent 覆盖正式事件处理；原生鼠标拖动另在 App 中验收。
    open();
    const order=()=>[...d.querySelectorAll('[data-source-option]')].map(el=>el.dataset.sourceOption);
    const handle=id=>$(`[data-source-drag="${id}"]`),row=id=>$(`[data-source-option="${id}"]`);
    record('九来源均有独立拖拽手柄',d.querySelectorAll('[data-source-drag]').length===9);
    if(!handle('codex'))throw new Error('来源拖拽手柄尚未实现');
    let pointer=0;
    const start=id=>{const r=handle(id).getBoundingClientRect();const token={pointerId:++pointer,clientX:r.left+r.width/2,clientY:r.top+r.height/2,button:0,bubbles:true,cancelable:true};handle(id).dispatchEvent(new w.PointerEvent('pointerdown',token));return token;};
    const end=id=>handle(id).dispatchEvent(new w.PointerEvent('pointercancel',{bubbles:true,pointerId:pointer}));
    const drop=(id,token,after)=>{const r=row(id).getBoundingClientRect(),point={...token,clientX:r.left+55,clientY:r.top+r.height*(after ? 0.75 : 0.25)};
      row(id).dispatchEvent(new w.PointerEvent('pointermove',point));return ()=>row(id).dispatchEvent(new w.PointerEvent('pointerup',point));};
    const selected=[...d.querySelectorAll('[data-source-visible]')].filter(el=>el.checked).map(el=>el.dataset.sourceVisible).sort().join();
    let transfer=start('codex'),finish=drop('claude-cli',transfer,false);
    record('拖动时显示前插入提示',row('claude-cli').classList.contains('drop-before'));
    finish();await paint();
    record('放下插入目标之前',order()[0]==='codex'&&order()[1]==='claude-cli');
    record('拖拽不修改勾选状态',[...d.querySelectorAll('[data-source-visible]')].filter(el=>el.checked).map(el=>el.dataset.sourceVisible).sort().join()===selected);
    record('未保存不改变正式 Tab',visible().join()===ids.join());
    $('#session-source-cancel').click();await paint();open();
    record('取消丢弃拖拽顺序',order().join()===ids.join());
    transfer=start('codex');drop('codex',transfer,false)();end('codex');await paint();
    record('同一行放下不改变顺序',order().join()===ids.join());
    transfer=start('codex');drop('claude-cli',transfer,false);end('codex');await paint();
    record('拖动取消不提交且清理提示',order().join()===ids.join()&&!$('.drop-before, .drop-after, .dragging'));
    transfer=start('codex');drop('claude-cli',transfer,false);
    const outside=$('#session-source-options').getBoundingClientRect();
    $('#session-source-options').dispatchEvent(new w.PointerEvent('pointerup',{...transfer,clientX:outside.right+50,clientY:outside.top+50}));await paint();
    record('列表外放下不提交排序',order().join()===ids.join()&&!$('.drop-before, .drop-after, .dragging'));
    record('列表外放下清理浮层',!$('.session-source-ghost'));
    transfer=start('codex');drop('claude-cli',transfer,false);handle('codex').dispatchEvent(new w.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));await paint();
    record('Escape 取消拖动并保留面板',$('#session-source-manager').open&&order().join()===ids.join()&&!$('.session-source-ghost'));
    const foreign=new w.DataTransfer();foreign.setData('text/plain','codex');row('claude-cli').dispatchEvent(new w.DragEvent('drop',{bubbles:true,cancelable:true,dataTransfer:foreign}));await paint();
    record('外部拖入不能修改顺序',order().join()===ids.join());
    transfer=start('codex');drop('gemini-cli',transfer,true)();await paint();
    record('支持向后跨多行插入',order().at(-1)==='codex'&&order().slice(0,-1).join()===ids.filter(id=>id!=='codex').join());
    await save();await load();
    record('拖拽保存后重载保留顺序',visible().at(-1)==='codex');
    frame.width=960;frame.height=600;await paint();open();$('#session-source-reset').click();await paint();
    $('#session-source-options').scrollTop=0;transfer=start('claude-cli');
    const bounds=$('#session-source-options').getBoundingClientRect();
    $('#session-source-options').dispatchEvent(new w.PointerEvent('pointermove',{...transfer,clientX:bounds.left+55,clientY:bounds.bottom-2}));
    record('窄窗口拖到边缘可滚动',$('#session-source-options').scrollTop>0);
    await paint();await paint();
    record('停留在边缘时持续滚动',$('#session-source-options').scrollTop>28);
    end('claude-cli');$('#session-source-cancel').click();await paint();
  } catch(error) {record('回归执行无异常',false);document.getElementById('results').textContent=String(error.stack||error);}
  finally {if(saved===null)localStorage.removeItem(key);else localStorage.setItem(key,saved);}
  const failures=results.filter(r=>!r.ok);
  document.getElementById('status').textContent=`完成：${results.length-failures.length}/${results.length} 通过`;
  document.getElementById('results').textContent+='\n'+JSON.stringify(failures,null,2);
  await fetch('/report?suite=preferences',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({results})});
  document.getElementById('run').disabled=false;
}
document.getElementById('run').addEventListener('click',run);
