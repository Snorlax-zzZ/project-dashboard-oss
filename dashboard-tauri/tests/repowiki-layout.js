/* 生产界面 + 合成 IPC：验证大量目录、长标题和真实 Markdown 的几何布局。 */
const frame = document.getElementById('app');
const results = [];
const paint = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
async function waitFor(predicate) {
  const start = performance.now();
  while (!predicate()) {
    if (performance.now() - start > 5000) throw new Error('等待 Wiki 页面超时');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
}
function record(name, ok, detail) { results.push({ name, ok: !!ok, detail }); }
async function run() {
  results.length = 0;
  document.getElementById('run').disabled = true;
  document.getElementById('status').textContent = '运行中：741 页合成 Wiki';
  try {
    await waitFor(() => frame.contentWindow.layoutBackend && frame.contentDocument.querySelector('#session-table tbody tr'));
    const w = frame.contentWindow, d = frame.contentDocument;
    const $ = selector => d.querySelector(selector);
    const rect = element => element.getBoundingClientRect();
    const pages = [{ page_id: 'home', title: '示例工程总览', path: 'wiki/home.md', parent_page_id: null, order: 0 }];
    for (let group = 0; group < 20; group++) {
      pages.push({ page_id: `module-${group}`, title: `example-service-commons-${group}：共享领域与基础设施`, path: `wiki/module-${group}.md`, parent_page_id: 'home', order: group });
      for (let child = 0; child < 36; child++) pages.push({ page_id: `page-${group}-${child}`, title: `LongModuleImplementation${child}：配置管理与业务调用说明`, path: `wiki/page-${group}-${child}.md`, parent_page_id: `module-${group}`, order: child });
    }
    const navigation = { schema_version: 2, home_page_id: 'home', pages };
    const originalInvoke = w.layoutBackend.invoke.bind(w.layoutBackend);
    const calls = [];
    let slowPageId='', releaseSlow;
    w.layoutBackend.invoke = async (command, args) => {
      calls.push(command);
      if (command === 'repowiki_read_page') {
        if (args.pageId===slowPageId) await new Promise(resolve=>{ releaseSlow=resolve; });
        const page = pages.find(p => p.page_id === args.pageId);
        const content = `# ${page.title}\n\n[返回总览](repowiki:home)\n\n跨**格式**搜索，函数 \`a.b()\` 与 <tag> 均为文本。\n\n` + Array.from({length:20}, (_, i) =>
          `## 章节 ${i + 1}\n\n正文说明：长文档应保留舒适的行距，滚动只影响阅读区。\n\n- 数据来源\n- 查询与分页\n\n| 字段 | 说明 |\n| --- | --- |\n| key | ${'long_source_path/'.repeat(15)} |\n\n\`\`\`text\n${'long_code_expression_'.repeat(30)}\n\`\`\`\n`).join('\n');
        return {protocol_version:1,command:'read page',ok:true,result:{...page,content,wiki_generation_id:'wiki-layout-fixture'},warnings:[]};
      }
      const response = await originalInvoke(command, args);
      if (command === 'repowiki_read_project') response.result.navigation = navigation;
      return response;
    };
    $('[data-tab="repowiki"]').click();
    await waitFor(() => $('.repowiki-nav-item[data-page-id="home"]'));
    for (const [width, height] of [[960,600],[1200,800],[1600,1000]]) {
      frame.width = width; frame.height = height;
      for (const mode of ['light','dark']) {
        if (d.documentElement.dataset.mode !== mode) $('#btn-theme-toggle').click();
        await paint();
        const label = `${width}×${height}/${mode}`;
        const toggle = $('.repowiki-nav-toggle[data-toggle-page-id="module-0"]');
        if (toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
        $('.repowiki-nav-item[data-page-id="page-0-0"]').click();
        await waitFor(() => $('.repowiki-markdown'));
        await paint();
        const item = $('.repowiki-nav-item[data-page-id="page-0-0"]');
        const nav = $('.repowiki-page-nav'), article = $('#repowiki-article'), shell = $('.rw-shell');
        const toggleRect = rect($('.repowiki-nav-toggle[data-toggle-page-id="module-0"]'));
        const parentRect = rect($('.repowiki-nav-item[data-page-id="module-0"]'));
        record(label+'/展开按钮与标题同排', Math.abs(toggleRect.y+toggleRect.height/2-parentRect.y-parentRect.height/2)<2);
        const chevron = $('.repowiki-nav-toggle[data-toggle-page-id="module-0"] svg');
        record(label+'/折叠符号清晰且点击区足够', toggleRect.width>=32 && toggleRect.height>=32 && chevron && rect(chevron).width>=18 && rect(chevron).height>=18);
        record(label+'/分组与文档有不同图标', !!$('.repowiki-nav-item[data-page-id="module-0"] .repowiki-node-icon.folder') && !!item.querySelector('.repowiki-node-icon.document'));
        record(label+'/长模块名与中文说明分行', $('.repowiki-nav-item[data-page-id="module-0"] .repowiki-nav-summary')?.textContent==='共享领域与基础设施');
        record(label+'/节点点击范围与文字空间足够', rect(item).height>=36 && rect(item.querySelector('.repowiki-nav-title')).width>=100, {height:rect(item).height,titleWidth:rect(item.querySelector('.repowiki-nav-title')).width});
        record(label+'/选中页与完整标题可识别', item.getAttribute('aria-current')==='page' && item.title===pages.find(p=>p.page_id==='page-0-0').title);
        record(label+'/目录行不超出边界', [...nav.querySelectorAll('.repowiki-nav-item')].every(node=>rect(node).right<=rect(nav).right));
        record(label+'/主区与正文无横向溢出', shell.scrollWidth<=shell.clientWidth+1 && article.scrollWidth<=article.clientWidth+1);
        record(label+'/目录与正文不重叠', rect(nav).right<=rect(article).left+1);
        const findBar=$('.repowiki-find-bar');
        if (findBar) {
          record(label+'/搜索工具栏不横向溢出',findBar.scrollWidth<=findBar.clientWidth+1 && [...findBar.querySelectorAll('input,button')].every(node=>rect(node).right<=rect(findBar).right));
          record(label+'/搜索输入与阅读区不重叠',rect(findBar).bottom<=rect(article).top+1 && rect(article).height>240);
        }
        record(label+'/目录可独立滚动', nav.scrollHeight>nav.clientHeight && /auto|scroll/.test(w.getComputedStyle(nav).overflowY));
        const beforeArticle = article.scrollTop;
        nav.scrollTop=120;
        record(label+'/滚动目录保持正文位置', nav.scrollTop>0 && article.scrollTop===beforeArticle);
        nav.scrollTop=0;
        const paragraphStyle=w.getComputedStyle($('.repowiki-markdown p'));
        record(label+'/正文行距可读', parseFloat(paragraphStyle.lineHeight)/parseFloat(paragraphStyle.fontSize)>=1.65);
        const linkStyle=w.getComputedStyle($('.wiki-page-link'));
        record(label+'/正文内链没有原生按钮底板', linkStyle.backgroundColor==='rgba(0, 0, 0, 0)' && parseFloat(linkStyle.borderTopWidth)===0);
        article.scrollTop=180;
        record(label+'/长正文可独立滚动', article.scrollTop>0);
        $('.repowiki-nav-item[data-page-id="page-0-1"]').click();
        await waitFor(() => $('.repowiki-article-head h1')?.textContent===pages.find(p=>p.page_id==='page-0-1').title);
        record(label+'/切页后正文回到顶部', article.scrollTop===0);
        const beforeCalls=calls.length;
        $('.repowiki-nav-toggle[data-toggle-page-id="module-0"]').focus();
        $('.repowiki-nav-toggle[data-toggle-page-id="module-0"]').click();
        record(label+'/折叠不读文件且隐藏子节点', calls.length===beforeCalls && !$('.repowiki-nav-item[data-page-id="page-0-0"]'));
        record(label+'/折叠后键盘焦点留在原控件', d.activeElement.dataset.togglePageId==='module-0');
        shell.scrollTop=shell.scrollHeight;
        await paint();
        const graphRect=rect($('.repowiki-graph-panel'));
        record(label+'/底部图查询仍可达', graphRect.bottom<=height+1 && graphRect.top>=0, graphRect.toJSON());
        shell.scrollTop=0;
      }
    }
    const navSearch = $('#repowiki-nav-query'), findInput = $('#repowiki-find-query');
    record('搜索/两栏具有独立搜索入口', navSearch && findInput);
    if (navSearch && findInput) {
      const input = async (node, value) => { node.value=value; node.dispatchEvent(new w.Event('input',{bubbles:true})); await new Promise(resolve=>setTimeout(resolve,230)); await paint(); };
      const countBefore=calls.length;
      await input(navSearch,'LongModuleImplementation35');
      record('搜索/包含折叠子页并保留祖先', !!$('.repowiki-nav-item[data-page-id="page-19-35"]') && !!$('.repowiki-nav-item[data-page-id="home"]') && !$('.repowiki-nav-item[data-page-id="page-0-0"]'));
      record('搜索/目录匹配数量准确', $('#repowiki-nav-count').textContent.includes('20'));
      record('搜索/目录筛选没有 IPC',calls.length===countBefore);
      navSearch.dispatchEvent(new w.CompositionEvent('compositionstart',{bubbles:true}));
      await input(navSearch,'输入法组合中');
      record('搜索/中文组合输入期间保留原目录',!!$('.repowiki-nav-item[data-page-id="page-19-35"]'));
      navSearch.dispatchEvent(new w.CompositionEvent('compositionend',{bubbles:true}));
      await waitFor(()=>$('#repowiki-page-nav').textContent.includes('没有匹配'));
      record('搜索/中文组合结束后更新结果', $('#repowiki-nav-count').textContent==='匹配 0 篇');
      await input(navSearch,'完全不存在');
      record('搜索/目录无匹配提示', $('#repowiki-page-nav').textContent.includes('没有匹配'));
      $('#btn-repowiki-nav-clear').click(); await paint();
      record('搜索/清空恢复折叠树', !$('.repowiki-nav-item[data-page-id="page-0-0"]') && !!$('.repowiki-nav-item[data-page-id="module-19"]'));
      $('.repowiki-nav-toggle[data-toggle-page-id="module-0"]').click();
      $('.repowiki-nav-item[data-page-id="page-0-0"]').click();
      await waitFor(()=>$('.repowiki-markdown'));
      const originalBody=$('.repowiki-markdown').innerHTML;
      const bodyCalls=calls.length;
      await input(findInput,'跨格式搜索');
      record('搜索/跨行内格式作为一处匹配', $('#repowiki-find-count').textContent==='1 / 1' && [...d.querySelectorAll('.repowiki-find-hit')].map(x=>x.textContent).join('')==='跨格式搜索');
      await input(findInput,'a.b()');
      record('搜索/特殊字符按字面匹配', $('#repowiki-find-count').textContent==='1 / 1' && $('.repowiki-find-hit')?.closest('code'));
      await input(findInput,'<tag>');
      record('搜索/标记样式文字不能变成 HTML', $('.repowiki-find-hit')?.textContent==='<tag>' && !$('.repowiki-markdown tag'));
      await input(findInput,'<img src=x onerror=alert(1)>');
      record('搜索/查找输入不能注入元素', $('#repowiki-find-count').textContent==='无匹配' && !$('.repowiki-markdown img'));
      await input(findInput,'正文说明');
      record('搜索/高亮与计数一致', $('#repowiki-find-count').textContent==='1 / 20' && d.querySelectorAll('.repowiki-find-hit').length===20);
      $('#btn-repowiki-find-prev').click();
      record('搜索/上一处循环至末尾且正文滚动', $('#repowiki-find-count').textContent==='20 / 20' && $('#repowiki-article').scrollTop>0);
      findInput.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));
      record('搜索/Enter 循环至首处', $('#repowiki-find-count').textContent==='1 / 20');
      findInput.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Enter',shiftKey:true,bubbles:true,cancelable:true}));
      record('搜索/Shift+Enter 循环至末尾', $('#repowiki-find-count').textContent==='20 / 20');
      await input(findInput,'正文说明');
      record('搜索/同词防抖回调不重置定位', $('#repowiki-find-count').textContent==='20 / 20');
      const composingText=$('#repowiki-find-count').textContent;
      findInput.dispatchEvent(new w.CompositionEvent('compositionstart',{bubbles:true}));
      await input(findInput,'文档组合中');
      record('搜索/中文组合输入不重置高亮', $('#repowiki-find-count').textContent===composingText);
      findInput.dispatchEvent(new w.CompositionEvent('compositionend',{bubbles:true}));
      await waitFor(()=>$('#repowiki-find-count').textContent==='无匹配');
      await input(findInput,'零匹配词');
      record('搜索/无匹配禁用跳转', $('#repowiki-find-count').textContent==='无匹配' && $('#btn-repowiki-find-next').disabled);
      findInput.dispatchEvent(new w.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));
      record('搜索/清空恢复正文结构', !$('.repowiki-find-hit') && $('.repowiki-markdown').innerHTML===originalBody);
      record('搜索/全文内查找没有 IPC',calls.length===bodyCalls);
      d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'f',metaKey:true,bubbles:true,cancelable:true}));
      record('搜索/快捷键定位文档输入框',d.activeElement===findInput);
      navSearch.focus();
      d.dispatchEvent(new w.KeyboardEvent('keydown',{key:'f',ctrlKey:true,bubbles:true,cancelable:true}));
      record('搜索/Ctrl+F 同样定位文档输入框',d.activeElement===findInput);
      await input(findInput,'正文说明');
      $('.wiki-page-link').click(); await waitFor(()=>$('.repowiki-article-head h1')?.textContent==='示例工程总览');
      record('搜索/切页清空旧匹配且内链有效', findInput.value==='' && !$('.repowiki-find-hit') && $('#repowiki-article').scrollTop===0);
      slowPageId='page-0-1';
      $('.repowiki-nav-item[data-page-id="page-0-1"]').click();
      record('搜索/读页过程中禁用文档搜索',findInput.disabled);
      $('.repowiki-nav-item[data-page-id="home"]').click();
      await waitFor(()=>$('.repowiki-article-head h1')?.textContent==='示例工程总览');
      await input(findInput,'正文说明');
      releaseSlow(); await paint();
      record('搜索/过期读页响应不能覆盖当前搜索', $('.repowiki-article-head h1')?.textContent==='示例工程总览' && $('#repowiki-find-count').textContent==='1 / 20');
      slowPageId='';
      await input(navSearch,'module'); await input(findInput,'正文说明');
      $('#btn-repowiki-refresh').click(); await waitFor(()=>$('.repowiki-nav-item[data-page-id="home"]'));
      record('搜索/重读项目清空两处搜索并禁用空文档查找',navSearch.value==='' && findInput.value==='' && findInput.disabled);
    }
  } catch(error) { record('测试执行异常',false,String(error.stack||error)); }
  const failed=results.filter(result=>!result.ok);
  document.getElementById('status').textContent=`完成：${results.length-failed.length}/${results.length} 通过，${failed.length} 失败`;
  document.getElementById('results').textContent=JSON.stringify(failed.length?failed:results,null,2);
  await fetch('/report?suite=repowiki',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({results})});
  document.getElementById('run').disabled=false;
}
document.getElementById('run').addEventListener('click',run);
