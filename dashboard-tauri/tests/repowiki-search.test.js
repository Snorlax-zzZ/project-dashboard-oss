const test = require('node:test');
const assert = require('node:assert/strict');
const ui = require('../dist/app.js');
const pages = [
  {page_id:'home',title:'首页'},
  {page_id:'group',title:'基础设施',parent_page_id:'home'},
  {page_id:'leaf',title:'Storage：缓存配置',parent_page_id:'group'},
  {page_id:'other',title:'其他模块',parent_page_id:'home'},
];

test('目录筛选命中折叠深层标题并只保留祖先', () => {
  const result=ui.filterRepoWikiNavigation(pages,' storage ');
  assert.deepEqual([...result.matches],['leaf']);
  assert.deepEqual([...result.visible].sort(),['group','home','leaf']);
  assert.deepEqual([...result.ancestors].sort(),['group','home']);
});
test('目录空白筛选恢复全树；无匹配与中文匹配明确', () => {
  assert.equal(ui.filterRepoWikiNavigation(pages,'   '),null);
  assert.equal(ui.filterRepoWikiNavigation(pages,'不存在').matches.size,0);
  assert.deepEqual([...ui.filterRepoWikiNavigation(pages,'缓存').matches],['leaf']);
});
test('非法父节点和循环不会卡死目录搜索', () => {
  const result=ui.filterRepoWikiNavigation([
    {page_id:'a',title:'命中',parent_page_id:'b'},
    {page_id:'b',title:'另一页',parent_page_id:'a'},
    {page_id:'c',title:'命中',parent_page_id:'missing'},
  ],'命中');
  assert.equal(result.visible.size,3);
  assert.equal(result.matches.size,2);
});
test('正文查找按字面匹配特殊字符且不区分大小写', () => {
  assert.deepEqual(ui.findRepoWikiTextRanges('A.b [x] a.B','a.b').ranges,[{start:0,end:3},{start:8,end:11}]);
  assert.deepEqual(ui.findRepoWikiTextRanges('<tag> <tag>','<tag>').ranges,[{start:0,end:5},{start:6,end:11}]);
  assert.equal(ui.findRepoWikiTextRanges('abc','.*').ranges.length,0);
});
test('中文、emoji 与非重叠匹配保留正确文本偏移', () => {
  assert.deepEqual(ui.findRepoWikiTextRanges('🌳缓存🌳缓存','缓存').ranges,[{start:2,end:4},{start:6,end:8}]);
  assert.deepEqual(ui.findRepoWikiTextRanges('aaaa','aa').ranges,[{start:0,end:2},{start:2,end:4}]);
  assert.equal(ui.findRepoWikiTextRanges('abc',' ').ranges.length,0);
});
test('大量正文匹配有高亮上限，并区分刚好到上限', () => {
  assert.equal(ui.findRepoWikiTextRanges('a'.repeat(1001),'a').ranges.length,1000);
  assert.equal(ui.findRepoWikiTextRanges('a'.repeat(1001),'a').truncated,true);
  assert.equal(ui.findRepoWikiTextRanges('a'.repeat(1000),'a').truncated,false);
});
