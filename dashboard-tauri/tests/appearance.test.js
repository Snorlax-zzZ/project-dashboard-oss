const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const scriptPath=path.join(__dirname,'../dist/appearance.js');

function launch({stored,systemDark=false,denyStorage=false}={}){
  assert.ok(fs.existsSync(scriptPath),'正式页面需要独立的外观控制器');
  const values=new Map(stored===undefined?[]:[['project-dashboard.appearance',stored]]);
  const callbacks=new Map(),clicks=[],ipc=[];
  const button={title:'',attrs:{},setAttribute(k,v){this.attrs[k]=v;},addEventListener(type,handler){assert.equal(type,'click');clicks.push(handler);}};
  const label={textContent:''},html={dataset:{}};
  const business={source:'codex',page:3,query:'缓存',drawer:{open:true,width:570},rows:[1,2,3]};
  const document={documentElement:html,readyState:'loading',addEventListener(type,fn){callbacks.set(type,fn);},getElementById(id){return id==='btn-theme-toggle'?button:id==='appearance-label'?label:null;}};
  const window={matchMedia:()=>({matches:systemDark}),__TAURI__:{core:{invoke:(...args)=>ipc.push(args)}}};
  Object.defineProperty(window,'localStorage',{get(){if(denyStorage)throw new Error('存储不可用');return {getItem:key=>values.get(key)||null,setItem:(key,value)=>values.set(key,value)};}});
  vm.runInNewContext(fs.readFileSync(scriptPath,'utf8'),{window,document,state:business});
  const beforeReady={...html.dataset};
  callbacks.get('DOMContentLoaded')?.();
  return {html,button,label,business,ipc,values,beforeReady,click(){assert.equal(clicks.length,1);clicks[0]();}};
}

test('首次绘制前恢复有效偏好，无效或缺失偏好采用系统明暗',()=>{
  assert.equal(launch({stored:'light',systemDark:true}).beforeReady.mode,'light');
  assert.equal(launch({stored:'dark',systemDark:false}).beforeReady.mode,'dark');
  assert.equal(launch({stored:'unknown',systemDark:true}).beforeReady.mode,'dark');
  assert.equal(launch({systemDark:false}).beforeReady.mode,'light');
});

test('切换更新按钮与持久化，保留列表详情状态且不调用 IPC',()=>{
  const app=launch({stored:'dark'});const before=JSON.stringify(app.business);
  assert.equal(app.button.attrs['aria-pressed'],'true');assert.equal(app.button.title,'切换到浅色');
  app.click();
  assert.equal(app.html.dataset.theme,'16');assert.equal(app.html.dataset.mode,'light');
  assert.equal(app.label.textContent,'浅色');assert.equal(app.button.attrs['aria-pressed'],'false');
  assert.equal(app.values.get('project-dashboard.appearance'),'light');
  assert.equal(JSON.stringify(app.business),before);assert.deepEqual(app.ipc,[]);
  assert.equal(launch({stored:app.values.get('project-dashboard.appearance'),systemDark:true}).beforeReady.mode,'light');
  app.click();assert.equal(app.html.dataset.theme,'19');assert.equal(app.html.dataset.system,'pill');
});

test('WebView 拒绝本机存储时仍能启动与切换',()=>{
  const app=launch({denyStorage:true,systemDark:true});
  assert.equal(app.beforeReady.mode,'dark');assert.doesNotThrow(()=>app.click());
  assert.equal(app.html.dataset.mode,'light');assert.equal(app.button.title,'切换到深色');
});

test('正式页面保留五个入口、外观开关独立且所有资源可离线打包',()=>{
  const dist=path.join(__dirname,'../dist');
  const html=fs.readFileSync(path.join(dist,'index.html'),'utf8');
  assert.ok(html.indexOf('src="./appearance.js"')<html.indexOf('href="./app.css"'));
  assert.deepEqual([...html.matchAll(/data-tab="([^"]+)"/g)].map(m=>m[1]),['sessions','fleet','projects','repowiki','settings']);
  const toggle=html.match(/<button[^>]*id="btn-theme-toggle"[^>]*>/)[0];
  assert.doesNotMatch(toggle,/nav-item|data-tab=/);
  assert.doesNotMatch(html,/mock-backend|preview-bridge|StyleThemes/);
  for(const [,file]of html.matchAll(/(?:src|href)="\.\/([^"]+)"/g))assert.ok(fs.existsSync(path.join(dist,file)),file);
  const css=fs.readFileSync(path.join(dist,'appearance.css'),'utf8');
  assert.doesNotMatch(css,/url\(["']?(?:https?:|data:)/);
  for(const [,file]of css.matchAll(/url\("\.\/([^"]+)"\)/g))assert.ok(fs.existsSync(path.join(dist,file)),file);
});
