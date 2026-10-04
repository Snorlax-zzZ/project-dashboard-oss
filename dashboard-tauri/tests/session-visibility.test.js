const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const app = fs.readFileSync(path.join(__dirname, '../dist/app.js'), 'utf8');
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise, resolve, reject}; };
const page = (items=[], current=1, total=items.length) => ({items, page:current, page_size:20, total});
function harness(invoke) {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) {
      const classes = new Set(), listeners = new Map();
      elements.set(id, {value:'',textContent:'',innerHTML:'',style:{},dataset:{},
        isConnected:true,
        classList:{add:(x)=>classes.add(x), remove:(x)=>classes.delete(x), contains:(x)=>classes.has(x), toggle:(x,on)=>on?classes.add(x):classes.delete(x)},
        focus(){ctx.document.activeElement=this;},
        addEventListener(type,handler){if(!listeners.has(type))listeners.set(type,new Set());listeners.get(type).add(handler);},
        removeEventListener(type,handler){listeners.get(type)?.delete(handler);},
        emit(type,event={}){for(const handler of [...(listeners.get(type)||[])])handler({target:this,...event});},
        setAttribute(){},querySelectorAll:()=>[]});
    }
    return elements.get(id);
  };
  const ctx = { console:{log(){},error(){}}, module:{exports:{}}, setTimeout,clearTimeout,
    document:{body:element('body'),addEventListener(){},getElementById:element,querySelectorAll:()=>[],querySelector:()=>null},
    window:{__TAURI__:{core:{invoke}}}, notices:[] };
  vm.createContext(ctx); vm.runInContext(app,ctx);
  const events=element('document-events');
  ctx.document.addEventListener=events.addEventListener;
  ctx.document.removeEventListener=events.removeEventListener;
  vm.runInContext("renderSessions = () => {}; renderPagination = () => {}; flash = (message, error) => notices.push({message,error});",ctx);
  return { ctx, element, press:key=>events.emit('keydown',{key,preventDefault(){},stopPropagation(){}}), run:code=>vm.runInContext(code,ctx) };
}
test('hide waits for confirmation and cancel paths preserve list and paging',async()=>{
  for(const cancel of ['button','Escape','Enter','backdrop']) {
    const calls=[];const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve(page());});
    h.run("activeSessionView().items=[{session_id:'target',favorited:true}];activeSessionView().page=3");
    const trigger=h.element('hide-trigger');trigger.focus();
    const pending=h.run("requestSessionHidden('claude-cli',{session_id:'target',title:'<test> title'},true)");
    assert.equal(h.element('confirm-backdrop').classList.contains('open'),true);
    assert.equal(calls.length,0);
    assert.equal(h.ctx.document.activeElement,h.element('confirm-cancel'));
    assert.match(h.element('confirm-message').textContent,/Claude Code[\s\S]*<test> title[\s\S]*target[\s\S]*不会删除[\s\S]*已隐藏/);
    if(cancel==='button')h.element('confirm-cancel').emit('click');
    else if(cancel==='backdrop')h.element('confirm-backdrop').emit('mousedown');
    else h.press(cancel);
    await pending;
    assert.equal(calls.length,0);assert.equal(h.run('activeSessionView().page'),3);
    assert.equal(h.run('activeSessionView().items[0].favorited'),true);
    assert.equal(h.ctx.document.activeElement,trigger);
  }
});
test('confirmation is single, traps focus and writes only captured identity after explicit accept',async()=>{
  const calls=[]; const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve(page());});
  const first=h.run("requestSessionHidden('claude-cli',{session_id:'one',title:'One'},true)");
  await h.run("requestSessionHidden('claude-cli',{session_id:'two',title:'Two'},true)");
  assert.match(h.element('confirm-message').textContent,/One/);assert.equal(calls.length,0);
  h.press('Tab');assert.equal(h.ctx.document.activeElement,h.element('confirm-ok'));
  h.press('Tab');assert.equal(h.ctx.document.activeElement,h.element('confirm-cancel'));
  h.press('Tab');h.press('Enter');await first;
  assert.deepEqual(calls.filter(c=>c.cmd==='set_session_hidden').map(c=>[c.source,c.sessionId,c.hidden]),[['claude-cli','one',true]]);
  assert.equal(h.element('confirm-backdrop').classList.contains('open'),false);
});
test('leaving the original view before accepting cancels the pending hide',async()=>{
  for(const change of ["state.sessionSource='pi'","activeSessionView().visibility='hidden'","state.activeTab='fleet'"]) {
    let writes=0;const h=harness(()=>{writes++;return Promise.resolve(page());});
    const pending=h.run("requestSessionHidden('claude-cli',{session_id:'target'},true)");
    h.run(change);h.element('confirm-ok').emit('click');await pending;
    assert.equal(writes,0);
  }
});
test('restore and favorite do not open a hide confirmation',async()=>{
  const calls=[]; const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve(page());});
  await h.run("requestSessionHidden('claude-cli',{session_id:'target'},false)");
  await h.run("toggleSessionFavorite('claude-cli','target')");
  assert.equal(h.element('confirm-backdrop').classList.contains('open'),false);
  assert.equal(calls.find(c=>c.cmd==='set_session_hidden').hidden,false);
  assert.equal(calls.filter(c=>c.cmd==='toggle_favorite').length,1);
});
test('Escape cancels the top confirmation without closing an existing detail drawer',async()=>{
  const h=harness(()=>{throw new Error('must not write');});
  h.run("bindDrawerClose();state.sessionDetailKey='claude-cli:target'");
  h.element('drawer').classList.add('open');
  const pending=h.run("requestSessionHidden('claude-cli',{session_id:'target'},true)");
  h.press('Escape');await pending;
  assert.equal(h.element('drawer').classList.contains('open'),true);
  assert.equal(h.run('state.sessionDetailKey'),'claude-cli:target');
});
test('existing confirmations still focus confirm by default and accept Enter',async()=>{
  const h=harness(()=>{throw new Error('must not write');});
  const pending=h.run("uiConfirm('既有项目操作确认')");
  assert.equal(h.ctx.document.activeElement,h.element('confirm-ok'));
  h.press('Enter');assert.equal(await pending,true);
});
test('project deletion defaults to cancel and cancel paths never write',async()=>{
  for(const cancel of ['Enter','Escape','button','backdrop']) {
    const calls=[];
    const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve();});
    h.run('refreshAll = async () => {}');
    const trigger=h.element('delete-trigger');trigger.focus();
    const pending=h.run("deleteProject('project-a','测试项目')");
    assert.equal(h.ctx.document.activeElement,h.element('confirm-cancel'));
    assert.match(h.element('confirm-message').textContent,/测试项目/);
    if(cancel==='button')h.element('confirm-cancel').emit('click');
    else if(cancel==='backdrop')h.element('confirm-backdrop').emit('mousedown');
    else h.press(cancel);
    await pending;
    assert.equal(calls.length,0);
    assert.equal(h.element('confirm-backdrop').classList.contains('open'),false);
    assert.equal(h.ctx.document.activeElement,trigger);
  }
});
test('project deletion still writes the selected project once after explicit confirmation',async()=>{
  const calls=[];
  const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve();});
  h.run('refreshAll = async () => {}');
  const pending=h.run("deleteProject('project-a','测试项目')");
  h.press('Tab');
  assert.equal(h.ctx.document.activeElement,h.element('confirm-ok'));
  h.press('Enter');h.press('Enter');await pending;
  assert.deepEqual(calls,[{cmd:'delete_project',projectId:'project-a'}]);
});
test('a newer confirmation cancels the hidden one so one click cannot approve two actions',async()=>{
  let writes=0;const h=harness(()=>{writes++;return Promise.resolve(page());});
  const hide=h.run("requestSessionHidden('claude-cli',{session_id:'target'},true)");
  const other=h.run("uiConfirm('异步返回的项目关联确认')");
  h.element('confirm-ok').emit('click');
  assert.equal(await other,true);await hide;
  assert.equal(writes,0);
});
test('confirmed hide returns keyboard focus to a stable control after the row is redrawn',async()=>{
  const h=harness(()=>Promise.resolve(page()));
  h.element('hide-trigger').focus();
  h.run('renderSessions = () => {document.activeElement = document.body;}');
  const pending=h.run("requestSessionHidden('claude-cli',{session_id:'target'},true)");
  h.element('confirm-ok').emit('click');await pending;
  assert.equal(h.ctx.document.activeElement,h.element('session-visibility'));
});
test('default visibility and per-source range switches preserve search and page size',async()=>{
  const calls=[]; const h=harness((cmd,args)=>{calls.push({cmd,...args});return Promise.resolve(page());});
  assert.equal(h.run('activeSessionView().visibility'),'visible');
  h.run("activeSessionView().page=4; activeSessionView().pageSize=50; state.sessionDetailKey='claude-cli:old'");
  h.element('session-query').value='needle';
  await h.run("switchSessionVisibility('hidden')");
  assert.equal(calls.at(-1).visibility,'hidden');
  assert.equal(calls.at(-1).query,'needle');
  assert.equal(calls.at(-1).page,1); assert.equal(calls.at(-1).pageSize,50);
  assert.equal(h.run('state.sessionDetailKey'),'');
  h.run("switchSessionSource('pi')");
  assert.equal(h.run('activeSessionView().visibility'),'visible');
  h.run("switchSessionSource('claude-cli')");
  assert.equal(h.element('session-visibility').value,'hidden');
});
test('hide invalidates requests from before and during save and keeps latest page intent',async()=>{
  const before=deferred(),during=deferred(),after=deferred(),save=deferred(); const calls=[];
  const responses=[before,during,after]; let i=0;
  const h=harness((cmd,args)=>{calls.push({cmd,...args});return cmd==='set_session_hidden'?save.promise:responses[i++].promise;});
  h.run("activeSessionView().items=[{session_id:'target'}]");
  const old=h.run('fetchSessions(1,20)');
  const mutation=h.run("setSessionHidden('claude-cli','target',true)");
  assert.equal(h.run("activeSessionView().pendingHidden.has('target')"),true);
  const middle=h.run('fetchSessions(2,20)');
  before.resolve(page([{session_id:'before'}])); await old;
  assert.equal(h.run('activeSessionView().items[0].session_id'),'target');
  save.resolve({source:'claude-cli',session_id:'target',hidden:true});
  await new Promise(r=>setImmediate(r));
  assert.equal(calls.at(-1).page,2);
  during.resolve(page([{session_id:'during'}],2)); await middle;
  after.resolve(page([{session_id:'latest'}],1,1)); await mutation;
  assert.equal(h.run('activeSessionView().items[0].session_id'),'latest');
  assert.equal(h.run('activeSessionView().page'),1);
  assert.equal(h.run("activeSessionView().pendingHidden.has('target')"),false);
});
test('write failure preserves row and distinguishes save from refresh failure',async()=>{
  const h=harness(()=>Promise.reject(new Error('disk full')));
  h.run("activeSessionView().items=[{session_id:'target'}]");
  await h.run("setSessionHidden('claude-cli','target',true)");
  assert.equal(h.run('activeSessionView().items[0].session_id'),'target');
  assert.match(h.run('activeSessionView().notice'),/保存失败/);
  assert.equal(h.run('activeSessionView().error'),'');
  const h2=harness(cmd=>cmd==='set_session_hidden'?Promise.resolve({hidden:true}):Promise.reject(new Error('scan failed')));
  await h2.run("setSessionHidden('claude-cli','target',true)");
  assert.match(h2.run('activeSessionView().notice'),/已保存.*刷新失败/);
});
test('completion after source switch does not refresh or close another source drawer',async()=>{
  const save=deferred(),calls=[];
  const h=harness((cmd,args)=>{calls.push({cmd,...args});return cmd==='set_session_hidden'?save.promise:Promise.resolve(page());});
  const mutation=h.run("setSessionHidden('claude-cli','target',true)");
  h.run("switchSessionSource('pi')");
  await new Promise(r=>setImmediate(r));
  h.run("state.sessionDetailKey='pi:another'; activeSessionView().page=3");
  const count=calls.length;
  save.resolve({hidden:true}); await mutation;
  assert.equal(calls.length,count);
  assert.equal(h.run('state.sessionDetailKey'),'pi:another');
  assert.equal(h.run('activeSessionView().page'),3);
  assert.equal(h.run("state.sessionViews['claude-cli'].needsRefresh"),true);
});
test('completion follows the latest visibility and closes only matching detail',async()=>{
  const save=deferred(),calls=[];
  const h=harness((cmd,args)=>{calls.push({cmd,...args});return cmd==='set_session_hidden'?save.promise:Promise.resolve(page());});
  h.run("state.sessionDetailKey='claude-cli:target'");
  const mutation=h.run("setSessionHidden('claude-cli','target',true)");
  await h.run("switchSessionVisibility('hidden')");
  save.resolve({hidden:true}); await mutation;
  assert.equal(calls.at(-1).visibility,'hidden');
  assert.equal(h.run('state.sessionDetailKey'),'');
});
test('duplicate clicks during the same mutation do not send duplicate writes',async()=>{
  const save=deferred();let writes=0;
  const h=harness(cmd=>{if(cmd==='set_session_hidden'){writes++;return save.promise;}return Promise.resolve(page());});
  const first=h.run("setSessionHidden('pi','same',true)");
  await h.run("setSessionHidden('pi','same',true)");
  assert.equal(writes,1);save.resolve({hidden:true});await first;
});
test('browser fallback uses persistent-in-session hiding with filtering before paging',async()=>{
  const ui=require('../dist/app.js');
  const before=await ui.mockInvoke('list_sessions',{source:'claude-cli',page:1,pageSize:1});
  const sid=before.items[0].session_id;
  await ui.mockInvoke('set_session_hidden',{source:'claude-cli',sessionId:sid,hidden:true});
  const visible=await ui.mockInvoke('list_sessions',{source:'claude-cli',page:999,pageSize:1});
  assert.equal(visible.total,before.total-1);
  const hidden=await ui.mockInvoke('list_sessions',{source:'claude-cli',visibility:'hidden',page:1,pageSize:1});
  assert.equal(hidden.total,1);assert.equal(hidden.items[0].session_id,sid);
  await ui.mockInvoke('set_session_hidden',{source:'claude-cli',sessionId:sid,hidden:false});
});
