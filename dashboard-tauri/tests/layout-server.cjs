// 有限时长的本机布局测试进程：只暴露白名单前端文件和合成 IPC，不访问真实用户数据。
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const dist = path.join(root, 'dashboard-tauri/dist');
const read = file => fs.readFileSync(file, 'utf8');
const files = new Map(['app.css', 'appearance.css', 'appearance.js', 'i18n.js', 'app.js', 'marked.min.js'].map(name => ['/' + name, path.join(dist, name)]));
for (const name of fs.readdirSync(path.join(dist, 'ui-icons'))) files.set('/ui-icons/' + name, path.join(dist, 'ui-icons', name));
files.set('/mock-backend.js', path.join(root, 'design/style-lab/mock-backend.js'));
files.set('/layout-browser.js', path.join(__dirname, 'layout-browser.js'));
files.set('/repowiki-layout.js', path.join(__dirname, 'repowiki-layout.js'));
files.set('/partner-layout.js', path.join(__dirname, 'partner-layout.js'));
files.set('/source-preferences-layout.js', path.join(__dirname, 'source-preferences-layout.js'));
files.set('/i18n-layout.js', path.join(__dirname, 'i18n-layout.js'));
for (const name of ['logo.svg']) files.set('/' + name, path.join(dist, name));
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  res.setHeader('Cache-Control', 'no-store');
  if (req.method === 'POST' && url.pathname === '/report') {
    let body = '';
    req.on('data', chunk => { body += chunk; if (body.length > 1024 * 1024) req.destroy(); });
    req.on('end', () => {
      try {
        const report = JSON.parse(body);
        if (!Array.isArray(report.results)) throw new Error('Invalid report');
        const out = path.join(root, 'log/layout-audit');
        fs.mkdirSync(out, {recursive:true});
        const name = url.searchParams.get('suite') === 'i18n' ? 'i18n-report.json' : url.searchParams.get('suite') === 'repowiki' ? 'repowiki-report.json' : url.searchParams.get('suite') === 'partners' ? 'partner-report.json' : url.searchParams.get('suite') === 'preferences' ? 'source-preferences-report.json' : 'layout-report.json';
        fs.writeFileSync(path.join(out, name), JSON.stringify(report, null, 2));
        res.end('saved');
      } catch (_) { res.writeHead(400); res.end('Invalid report'); }
    });
    return;
  }
  if (url.pathname === '/app.html') {
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    return res.end(read(path.join(dist, 'index.html')).replace('<script src="./app.js"></script>', `<script src="/mock-backend.js"></script><script>
      window.layoutBackend = createPreviewBackend();
      layoutBackend.setScenario(new URLSearchParams(location.search).get('scenario') || 'normal');
      window.__TAURI__ = {core:{invoke:(cmd,args)=>layoutBackend.invoke(cmd,args)}};
    </script><script src="./app.js"></script>`));
  }
  if (url.pathname === '/' || url.pathname === '/repowiki.html' || url.pathname === '/partners.html' || url.pathname === '/preferences.html' || url.pathname === '/i18n.html') {
    const script = url.pathname === '/i18n.html' ? 'i18n-layout.js' : url.pathname === '/repowiki.html' ? 'repowiki-layout.js' : url.pathname === '/partners.html' ? 'partner-layout.js' : url.pathname === '/preferences.html' ? 'source-preferences-layout.js' : 'layout-browser.js';
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    return res.end(`<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>正式前端布局回归</title>
      <style>body{margin:16px;font:14px system-ui;background:#eee;color:#222}button{padding:10px}pre{white-space:pre-wrap}iframe{display:block;border:1px solid #aaa;margin-top:12px;background:white}</style>
      <button id="run">运行全部布局回归</button> <span id="status">未运行 · 正式 HTML/CSS/JS，合成 IPC</span><pre id="results"></pre>
      <iframe id="app" width="1200" height="800" src="/app.html"></iframe><script src="/${script}"></script>`);
  }
  const file = files.get(url.pathname);
  if (!file) { res.writeHead(404); return res.end('Not found'); }
  res.setHeader('Content-Type', file.endsWith('.css') ? 'text/css' : file.endsWith('.svg') ? 'image/svg+xml' : file.endsWith('.png') ? 'image/png' : 'text/javascript');
  res.end(fs.readFileSync(file));
});
server.listen(Number(process.argv[2]) || 0, '127.0.0.1', () => console.log(`Layout test: http://127.0.0.1:${server.address().port}`));
// 测试结束可 Ctrl-C 提前关闭；无人操作也会自动退出，不留后台服务。
setTimeout(() => server.close(), Number(process.env.DASHBOARD_LAYOUT_TEST_TIMEOUT_MS) || 10 * 60 * 1000).unref();
