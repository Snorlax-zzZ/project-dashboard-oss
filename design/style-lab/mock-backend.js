/* 预览专用内存 IPC：只含合成数据，不访问磁盘、真实 Tauri 或网络。 */
(function (root) {
  'use strict';
  function createPreviewBackend() {
    const now = Date.now();
    const copy = value => JSON.parse(JSON.stringify(value));
    const iso = minutes => new Date(now - minutes * 60000).toISOString();
    const mac = 'mbp-demo01', win = 'win-demo02';
    const sourceCounts = { 'claude-cli': 143, codex: 126, pi: 67, grok: 52, opencode:83, antigravity:64, dsh:71, zcode:69, 'gemini-cli':75 };
    const prefixes = { 'claude-cli':'claude --resume ', codex:'codex resume ', pi:'pi --session ', grok:'grok --resume ', opencode:'opencode --session ', antigravity:'agy --conversation ' };
    const names = ['project-dashboard','atlas-api','repowiki','design-system','fleet-agent','notes-engine','docs-site','task-runner'];
    const titles = ['核对会话分页与来源隔离','修复缓存索引更新','梳理项目注册表逻辑','补齐错误状态提示','检查本地数据读取','收敛组件间距与字号','验证跨机快照聚合','整理代码知识导航','完善配置校验提示','处理长路径与表格滚动','优化最近提交展示','回归会话恢复命令'];
    let projects = { shared:names.map((name,i)=>({project_id:`a000000${i}-d123-4000-8000-1234567890ab`,name,git_url:`https://github.com/demo/${name}`,machines:{[mac]:`~/code/${name}`,[win]:`D:\\code\\${name}`},sync:true})), private:[{project_id:null,name:'private-scratch',git_url:'https://github.com/demo/private-scratch',path:'~/code/private-scratch',sync:false}] };
    const sessions = Object.fromEntries(Object.entries(sourceCounts).map(([source,count],si)=>[source,Array.from({length:count},(_,i)=>({
      session_id:`${(si+1).toString(16)}${(i+1).toString(16).padStart(7,'0')}-7a12-4000-9000-${(i+1).toString(16).padStart(12,'0')}`,
      source, title:i===12?'':`${titles[i%titles.length]}${i>=12?' · '+(Math.floor(i/12)+1):''}`,
      projects:names[i%names.length],cwd:`~/code/${names[i%names.length]}`,mtime:now/1000-180-si*90-i*1560,
      haystack:`${titles[i%titles.length]} ${i%3===0?'缓存命中 增量扫描 分页切片':'只读验证 项目配置'} 来源 ${source}`,
      favorited:i===2||i===8,size:38400+i*128,path:`/demo/${source}/${i}.jsonl`,
    }))]));
    let settings={schema_version:1,machine_id:mac,diagnostic_mode:false,dashboard_state_remote:'git@github.com:demo/dashboard-state.git',dashboard_state_root:'',repowiki_cli_path:'/opt/demo/bin/repowiki',repowiki_data_root:'/demo/repowiki-data'};
    const hiddenBySource = Object.fromEntries(Object.keys(sourceCounts).map(source => [source, new Set()]));
    let scenario='normal', migrated=false, counter=0;
    let syncStatus={last_success_at:null,last_attempt:null};
    const calls=[];
    projects.shared[5].machines['linux-demo03']='~/code/notes-engine';
    const navigation={schema_version:2,home_page_id:'home',layout_version:3,pages:[
      {page_id:'home',title:'项目总览',path:'wiki/home.md',page_type:'home',parent_page_id:null,order:0},
      {page_id:'architecture',title:'系统架构',path:'wiki/architecture.md',page_type:'section',parent_page_id:'home',order:1},
      {page_id:'sessions',title:'会话采集与缓存',path:'wiki/sessions.md',page_type:'module',parent_page_id:'home',order:2},
      {page_id:'modules',title:'模块说明',path:'wiki/modules/index.md',page_type:'section',parent_page_id:'home',order:3},
      {page_id:'core',title:'核心引擎',path:'wiki/modules/core.md',page_type:'module',parent_page_id:'modules',order:1},
      {page_id:'frontend',title:'桌面前端',path:'wiki/modules/frontend.md',page_type:'module',parent_page_id:'modules',order:2},
      {page_id:'contracts',title:'接口与数据契约',path:'wiki/contracts.md',page_type:'section',parent_page_id:'home',order:4},
    ]};
    const bodies={
      home:'# 项目总览\n\nproject-dashboard 汇总本机会话历史、跨机项目状态与代码知识。\n\n## 五个工作入口\n\n| 入口 | 数据 | 展示方式 |\n| --- | --- | --- |\n| 会话看板 | 来源会话元数据 | 搜索、分页表格、详情抽屉 |\n| 协作看板 | registry 与 snapshot | 分组表格与各机快照 |\n| 项目管理 | 共享与私有项目 | 表格与登记表单 |\n| 代码知识 | 已发布 Wiki 与图谱 | 只读导航和按需查询 |\n| 设置 | 本机配置 | 表单与身份检查 |\n\n[阅读系统架构](repowiki:architecture) · [会话采集与缓存](repowiki:sessions)\n\n> 本文是为样式预览编写的示例文档。',
      architecture:'# 系统架构\n\n## 读取路径\n\n桌面前端通过固定的 IPC 调用 Rust 引擎。会话查询按来源选择采集器，利用本地缓存减少重复解析。\n\n```mermaid\nflowchart LR\n A[桌面前端] --> B[固定 IPC]\n B --> C[Rust 引擎]\n C --> D[本地会话缓存]\n C --> E[项目快照]\n```\n\n前端只渲染当前页；详情和 Wiki 正文按用户操作加载。\n\n[返回总览](repowiki:home)',
      sessions:'# 会话采集与缓存\n\n## 列表与详情分开读取\n\n1. 选择 Claude Code、Codex、pi 或 Grok 来源。\n2. 来源扫描器检查缓存与文件变化。\n3. 后端按收藏、时间排序，全文检索后分页切片。\n4. 前端收到 `total / page / page_size / items`。\n5. 点击行后，单独读取最近五轮对话。\n\n```rust\nlist_sessions_paged_for_source(source, query, page, page_size)\n```\n\n### 保持效率的边界\n\n列表不预载全部会话正文，不将所有来源混入一个无限滚动列表。\n\n[接口契约](repowiki:contracts)',
      modules:'# 模块说明\n\n- [核心引擎](repowiki:core)：来源扫描、缓存、分页、跨机聚合。\n- [桌面前端](repowiki:frontend)：列表与表单渲染、用户事件和只读展示。',
      core:'# 核心引擎\n\n`dashboard-core` 提供按来源查询会话、聚合快照和读取配置的能力。\n\n## 分页查询\n\n每页由后端切片后返回，列表不携带整个对话历史。\n\n[会话采集](repowiki:sessions)',
      frontend:'# 桌面前端\n\n使用原生 HTML、CSS 与 JavaScript。\n\n本次样式预览保留现有 DOM、导航、分页、抽屉和表单，仅覆盖视觉变量及图标。',
      contracts:'# 接口与数据契约\n\n```json\n{"source":"codex","query":null,"page":2,"pageSize":20}\n```\n\n返回字段包含总数、页码、页大小和当前页条目。来源不匹配的详情请求会失败。\n\n> 浏览器预览中的这些数据均为合成样例。',
    };
    const envelope=(command,result)=>({protocol_version:1,command,ok:true,result,warnings:[]});
    const graphNodes=[
      {node_id:'a'.repeat(64),qualified_name:'commands::list_sessions_paged_for_source',name:'list_sessions_paged_for_source',kind:'function',path:'dashboard-core/src/commands.rs',line:97},
      {node_id:'b'.repeat(64),qualified_name:'sessions::list_paged',name:'list_paged',kind:'function',path:'dashboard-core/src/sessions.rs',line:1860},
      {node_id:'c'.repeat(64),qualified_name:'sessions::scan_codex_at',name:'scan_codex_at',kind:'function',path:'dashboard-core/src/sessions.rs',line:870},
    ];
    function fleet() {
      if(scenario==='empty')return [];
      const kinds=['one-in-progress','synced','insufficient','dual-dirty','single-machine','insufficient','synced','synced'];
      return projects.shared.map((p,i)=>{
        const kind=kinds[i%kinds.length];
        const machines={};
        for(const mid of Object.keys(p.machines)) {
          if(kind==='single-machine'&&mid!==mac) continue;
          const stale=i===7&&mid===win;
          const dirty=(i===0&&mid===mac)?7:(kind==='dual-dirty'?2:0);
          machines[mid]={machine_updated_at:iso(stale?1740:8),project:{project_id:p.project_id,name:p.name,branch:'main',head:((i===2||i===5)&&mid===win?'b':'a').repeat(40),upstream:'origin/main',dirty,untracked:dirty?1:0,ahead:i===2&&mid===mac?2:0,behind:0,collected_at:iso(stale?1740:8),last_commit_at:iso(42+i*31),dangerous_state:null,progress:i===0?{done:12,total:16,next:'检查分页与缓存命中'}:null,recent_commits:Array.from({length:5},(_,n)=>({sha_full:(i*100+n+20).toString(16).padStart(40,'a'),subject:['完善来源筛选与分页','修复缓存元数据读取','补齐项目登记校验','调整会话列表字段','更新验证用例'][n],time:iso(42+n*135),files:[{path:'src/module.rs',additions:12+n,deletions:3}]}))}};
        }
        return {project_id:p.project_id,name:p.name,git_url:p.git_url,machines,stale:i===7,verdict:{kind,machine_id:mac,latest_machine_id:mac,behind_machine_id:win,reason:i===5?'multi-machine':'head-mismatch'}};
      });
    }
    function lookupSession(args) {
      const found=sessions[args.source]?.find(s=>s.session_id===args.sessionId);
      if(!found)throw new Error('来源与 session id 不匹配');
      return found;
    }
    function handle(cmd,args) {
      switch(cmd){
        case 'get_local_context':return {machine_id:mac,machine_id_error:null,hostname:'DEMO-MAC',os:'macos',version:'0.2.9 · 样式预览'};
        case 'list_sessions':{
          if(!Object.hasOwn(sessions,args.source))throw new Error('不支持的会话来源');
          if(scenario==='error')throw new Error('演示：会话缓存读取失败，点击重新读取可重试');
          const visibility=args.visibility||'visible';
          if(!['visible','hidden'].includes(visibility))throw new Error('无效显示范围');
          const query=(args.query||'').trim().toLowerCase();
          const items=scenario==='empty'?[]:sessions[args.source].filter(s=>hiddenBySource[args.source].has(s.session_id)===(visibility==='hidden')&&(!query||[s.title,s.cwd,s.projects,s.session_id,s.haystack].some(v=>v.toLowerCase().includes(query)))).sort((a,b)=>Number(b.favorited)-Number(a.favorited)||b.mtime-a.mtime);
          const page_size=Math.max(1,Math.min(500,args.pageSize||20));
          const page=Math.max(1,Math.min(args.page||1,Math.max(1,Math.ceil(items.length/page_size))));
          return {page,page_size,total:items.length,items:items.slice((page-1)*page_size,page*page_size)};
        }
        case 'set_session_hidden':{
          if(!hiddenBySource[args.source])throw new Error('不支持的会话来源');
          if(args.hidden)hiddenBySource[args.source].add(args.sessionId);
          else hiddenBySource[args.source].delete(args.sessionId);
          return {source:args.source,session_id:args.sessionId,hidden:args.hidden};
        }
        case 'toggle_favorite':{
          const s=(sessions[args.source]||[]).find(s=>s.session_id===args.sessionId);
          if(!s)throw new Error('会话不存在');
          s.favorited=!s.favorited; return s.favorited;
        }
        case 'session_detail':{
          const s=lookupSession(args);
          if(s.source==='antigravity')return {preview:[],files:[],summary:'这是合成的会话摘要，用于验证布局。',notice:'此工具目前仅支持会话摘要，暂不支持完整对话。'};
          return {preview:[`🧑 帮我检查 ${s.projects} 的当前实现，重点看「${s.title||'会话缓存'}」。`,'🤖 我会先核对数据读取、分页参数和界面状态，保留已有功能。','🧑 切换来源时要保留各自的搜索词和页码。','🤖 已确认每个来源维护独立的 `query / page / pageSize / total / items`，请求返回后也会核对来源与请求序号。','🧑 全文搜索也要保持，不要只搜索这一页。','🤖 搜索由后端针对缓存元数据与检索字段执行，过滤后再分页。前端不会对当前页再次过滤。','🧑 详情里展示哪些内容？','🤖 点击行后读取最近 **5 轮对话**、改动文件和完整会话 ID。恢复命令按来源生成。','🧑 好，先保留这些交互，只调整外观。','🤖 已整理检查结果。\n\n- 四来源状态独立\n- 列表分页读取\n- 详情按需加载\n- 空态与错误态保留\n\n这是一段用于排版与滚动验证的示例对话。'],files:s.source==='grok'?[]:['dashboard-core/src/sessions.rs','dashboard-tauri/dist/app.js','dashboard-tauri/dist/app.css']};
        }
        case 'copy_resume_command':lookupSession(args);if(!prefixes[args.source])throw new Error('RESUME_UNSUPPORTED');return prefixes[args.source]+args.sessionId;
        case 'list_projects':return projects;
        case 'aggregate_fleet_board':if(scenario==='error')throw new Error('演示：snapshot 读取失败');return fleet();
        case 'get_settings':return settings;
        case 'update_settings':settings={...settings,...args.update};return {config:settings,sync:{state:settings.dashboard_state_remote?'synced':'unconfigured'}};
        case 'update_repowiki_settings':settings.repowiki_cli_path=args.repowikiCliPath;settings.repowiki_data_root=args.repowikiDataRoot;return settings;
        case 'open_data_dir':
        case 'open_session_data_dir':return null;
        case 'get_sync_status':return copy(syncStatus);
        case 'manual_sync':{
          const completed_at=new Date().toISOString();
          const outcome=scenario==='unconfigured'||!settings.dashboard_state_remote?{status:'unconfigured'}:{status:'synced',pushed:scenario!=='push-failed'};
          const status=outcome.status==='unconfigured'?'unconfigured':outcome.pushed?'succeeded':'push-failed';
          syncStatus={last_success_at:status==='succeeded'?completed_at:syncStatus.last_success_at,last_attempt:{completed_at,outcome:status}};
          return {collected_at:completed_at,outcome};
        }
        case 'list_known_machines':return [{machine_id:mac,hostname:'DEMO-MAC',is_local:true,is_legacy:false},{machine_id:win,hostname:'Workstation',is_local:false,is_legacy:false},{machine_id:'DEMO-MAC-Old',hostname:'DEMO-MAC-Old',is_local:false,is_legacy:true}];
        case 'pull_registry_before_write':return null;
        case 'find_project_by_git_url':{
          const url=String(args.gitUrl||'').replace(/\.git$/,'').toLowerCase();
          const p=projects.shared.find(p=>p.git_url.replace(/\.git$/,'').toLowerCase()===url);
          return p?{kind:'existing',project_id:p.project_id,name:p.name,machines:Object.keys(p.machines)}:{kind:'not-found',git_url_normalized:url};
        }
        case 'add_project':{
          const req=args.req;
          if(!req.name||!Object.values(req.machines||{}).some(Boolean))throw new Error('项目名与至少一个机器路径必填');
          const existing=projects.shared.find(p=>p.git_url.replace(/\.git$/,'').toLowerCase()===req.git_url.replace(/\.git$/,'').toLowerCase());
          if(req.sync&&existing){existing.machines={...existing.machines,...req.machines};return existing;}
          if([...projects.shared,...projects.private].some(p=>p.name===req.name))throw new Error('项目名已存在');
          const p=req.sync?{...req,project_id:`d0000000-0000-4000-8000-${(++counter).toString().padStart(12,'0')}`}:{name:req.name,git_url:req.git_url,path:req.path,sync:false,project_id:null}; projects[req.sync?'shared':'private'].push(p);return p;
        }
        case 'edit_project':{
          const p=projects.shared.find(p=>p.project_id===args.req.project_id);
          if(!p)throw new Error('项目不存在');Object.assign(p,args.req);return p;
        }
        case 'delete_project':projects={shared:projects.shared.filter(p=>p.project_id!==args.projectId),private:projects.private};return null;
        case 'plan_identity_migration':return {target_machine_id:mac,current_hostname:'DEMO-MAC',current_frozen_id:'DEMO-MAC-Old',claims:[{machine_id:'DEMO-MAC-Old',reason:{kind:'frozen_in_local_config'},has_snapshot_dir:true}],skipped:[{machine_id:win,reason:'属于另一台机器，原样保留'}],registry_edits:migrated?[]:[{project_name:'project-dashboard',from_key:'DEMO-MAC-Old',to_key:mac,path:'~/code/project-dashboard'}],snapshot_merge:null,conflicts:[],is_noop:migrated};
        case 'apply_identity_migration':migrated=true;return {registry_files_written:1,snapshot_written:false,committed:true,backup_path:'/demo/backups/identity-preview',legacy_dirs_kept:['DEMO-MAC-Old']};
        case 'repowiki_status':{
          if(scenario==='wiki-unconfigured'||!settings.repowiki_cli_path)throw new Error('[repowiki_cli_not_configured] 未配置 RepoWiki CLI 路径');
          return envelope('status',{initialization:'initialized',repo_id:'rwrepo_demo',repository:{branch_ref:'refs/heads/main',head_commit:'a'.repeat(40)},wiki:{freshness:'current',integrity:'healthy',generation_id:'wiki-demo-20261002'},graph:{availability:'ready',integrity:'healthy',source_freshness:scenario==='wiki-stale'?'stale':'current',generation_id:'graph-demo-08'},review:{totals:{total:7,reviewed:5,unreviewed:1,stale:1}}});
        }
        case 'repowiki_read_project':return envelope('read project',{repo_id:'rwrepo_demo',repository:{branch_ref:'refs/heads/main',head_commit:'a'.repeat(40)},wiki:{generation_id:'wiki-demo-20261002'},navigation});
        case 'repowiki_read_generations':return envelope('read generations',{branch:{branch_ref:'refs/heads/main'},wiki:{generation_id:'wiki-demo-20261002',status:'complete',integrity:'healthy',layout_version:3,mode:'update',generated_at:iso(80),head_commit:'a'.repeat(40),graph_generation_id:'graph-demo-08',page_total:7},review:{totals:{total:7,reviewed:5,unreviewed:1,stale:1},pages:{home:'reviewed',architecture:'reviewed',sessions:'reviewed',modules:'reviewed',core:'stale',frontend:'unreviewed',contracts:'reviewed'}}});
        case 'repowiki_read_page':{
          const p=navigation.pages.find(p=>p.page_id===args.pageId);if(!p)throw new Error('页面不存在');
          return envelope('read page',{...p,content:bodies[p.page_id],content_sha256:'2'.repeat(64),evidence_fingerprint:'3'.repeat(64),wiki_generation_id:'wiki-demo-20261002'});
        }
        case 'repowiki_graph_search':return envelope('graph search',{query:args.text,nodes:graphNodes.filter(n=>[n.name,n.path,n.qualified_name].some(v=>v.toLowerCase().includes((args.text||'').toLowerCase())))});
        case 'repowiki_graph_show':return envelope('graph show',{node:graphNodes.find(n=>n.node_id===args.nodeId),edges:[],truncated:false});
        case 'repowiki_graph_callers':case 'repowiki_graph_callees':case 'repowiki_graph_impact':return envelope('graph '+cmd.split('_').pop(),{nodes:graphNodes.filter(n=>n.node_id!==args.nodeId).map(n=>({...n,hop:1})),edges:[{source_id:'a'.repeat(64),target_id:'b'.repeat(64),type:'CALLS'}],truncated:false});
        default:throw new Error('预览未覆盖的 IPC：'+cmd);
      }
    }
    return {setScenario(value){scenario=value;},get calls(){return copy(calls);},async invoke(cmd,args={}){calls.push({cmd,args:copy(args||{})});return copy(handle(cmd,args||{}));}};
  }
  if(typeof module!=='undefined'&&module.exports)module.exports={createPreviewBackend};
  else root.createPreviewBackend=createPreviewBackend;
})(typeof window==='undefined'?globalThis:window);
