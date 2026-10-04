const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../dist/app.js'), 'utf8');
const deferred = () => { let resolve; const promise = new Promise(r => resolve = r); return {promise, resolve}; };
function boot(getStatus, sync = async () => ({})) {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, {value:'',textContent:'',disabled:false,
      classList:{add(){},remove(){}},addEventListener(){}});
    return elements.get(id);
  };
  const ctx = {module:{exports:{}}, console, setTimeout, clearTimeout,
    document:{addEventListener(){},getElementById:element},
    window:{__TAURI__:{core:{invoke:async(cmd)=> {
      if(cmd==='get_sync_status')return getStatus();
      if(cmd==='manual_sync')return sync();
      if(cmd==='list_projects')return {shared:[],private:[]};
      if(cmd==='aggregate_fleet_board')return [];
      throw new Error(cmd);
    }}}}};
  vm.createContext(ctx); vm.runInContext(source,ctx);
  vm.runInContext('fetchSessions = async () => {}; renderProjects = () => {}; renderFleetBoard = () => {};',ctx);
  return {run:code=>vm.runInContext(code,ctx), text:()=>element('sync-status').textContent};
}
const success = at => ({last_success_at:at,last_attempt:{completed_at:at,outcome:'succeeded'}});

test('fresh frontend contexts restore actual backend sync history on refresh',async()=>{
  const saved=success(new Date().toISOString());
  for(let i=0;i<2;i++) {
    const h=boot(async()=>saved); await h.run('refreshAll()');
    assert.match(h.text(),/跨机同步.*0 分钟前/);
    assert.doesNotMatch(h.text(),/尚未同步过|暂无同步记录/);
  }
});
test('a persisted failed attempt wins over an older success after restart',async()=>{
  const h=boot(async()=>({last_success_at:new Date(Date.now()-60000).toISOString(),
    last_attempt:{completed_at:new Date().toISOString(),outcome:'push-failed'}}));
  await h.run('refreshAll()'); await h.run('refreshAll()');
  assert.match(h.text(),/推送失败/); assert.doesNotMatch(h.text(),/分钟前/);
});
test('history read failures remain explicit across ordinary refreshes',async()=>{
  const h=boot(async()=>{throw new Error('记录损坏');});
  await h.run('refreshAll()'); await h.run('refreshAll()');
  assert.match(h.text(),/同步记录读取失败/); assert.doesNotMatch(h.text(),/尚未同步过|暂无同步记录/);
});
test('old asynchronous reads cannot replace a newer sync status',async()=>{
  const old=deferred(); let calls=0;
  const h=boot(()=>++calls===1?old.promise:Promise.resolve(success(new Date().toISOString())));
  const first=h.run('refreshAll()');
  await new Promise(setImmediate);
  await h.run('refreshAll()');
  old.resolve({last_success_at:null,last_attempt:null}); await first;
  assert.match(h.text(),/跨机同步.*0 分钟前/);
});
test('ordinary refresh cannot clear a running manual sync',async()=>{
  const pending=deferred(); const h=boot(async()=>({last_success_at:null,last_attempt:null}),()=>pending.promise);
  const work=h.run('manualSync()'); await h.run('refreshAll()');
  const during=h.text();
  pending.resolve({outcome:{status:'unconfigured'}}); await work;
  assert.match(during,/同步中/);
  assert.match(h.text(),/未配置/);
});
test('switching to a store with no history clears the previous store notice',async()=>{
  let record={last_success_at:null,last_attempt:{outcome:'unconfigured'}};
  const h=boot(async()=>record); await h.run('refreshAll()');
  assert.match(h.text(),/未配置/);
  record={last_success_at:null,last_attempt:null}; await h.run('refreshAll()');
  assert.match(h.text(),/暂无同步记录/);
});
test('a manual failure is durable through the next backend refresh',async()=>{
  let record=success(new Date().toISOString());
  const h=boot(async()=>record,async()=>{
    record={...record,last_attempt:{completed_at:new Date().toISOString(),outcome:'failed'}};
    throw new Error('pull failed');
  });
  await h.run('refreshAll()'); await h.run('manualSync()');
  assert.match(h.text(),/跨机同步失败/);
  await h.run('refreshAll()'); assert.match(h.text(),/跨机同步失败/);
});
test('a failed record write remains visible instead of restoring older success',async()=>{
  const old=success(new Date(Date.now()-600000).toISOString());
  const h=boot(async()=>old,async()=>({outcome:{status:'synced',pushed:true},
    collected_at:new Date().toISOString(),sync_record_warning:'同步记录保存失败'}));
  await h.run('manualSync()'); await h.run('refreshAll()');
  assert.match(h.text(),/同步记录保存失败/); assert.doesNotMatch(h.text(),/10 分钟前/);
});
test('a read started before manual sync cannot overwrite the completed result',async()=>{
  const old=deferred(); let calls=0; let record=null;
  const h=boot(()=>++calls===1?old.promise:Promise.resolve(record),async()=>{
    record=success(new Date().toISOString());
    return {outcome:{status:'synced',pushed:true},collected_at:record.last_success_at};
  });
  const first=h.run('refreshAll()'); await new Promise(setImmediate);
  await h.run('manualSync()'); old.resolve({last_success_at:null,last_attempt:null}); await first;
  assert.match(h.text(),/跨机同步.*0 分钟前/);
});
