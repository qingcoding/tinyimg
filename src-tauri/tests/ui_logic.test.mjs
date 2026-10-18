// 前端逻辑单测（Node 直接运行，无测试框架）：
//   node tests/ui_logic.test.mjs
// 覆盖 util.js 纯函数、ipc.js 的 Channel 有序投递 / end 清理 / 事件监听，
// 以及 i18n.js 的语言探测、归一化、插值、字典一致性与主题清单。
// app.js 依赖 DOM 不在本测试范围（由应用运行期验证）。

import assert from 'node:assert/strict';
import { fmtSize, esc, baseName, dirName, ratioText } from '../ui/util.js';
import { makeChannel, onTauriEvent, invoke } from '../ui/ipc.js';
import {
  LOCALES,
  FALLBACK_LOCALE,
  THEME_IDS,
  DICTS,
  normalizeLocale,
  detectLocale,
  createTranslator,
} from '../ui/i18n.js';

let passed = 0;
function test(name, fn) {
  try {
    fn();
    passed += 1;
    console.log(`  ✓ ${name}`);
  } catch (err) {
    console.error(`  ✗ ${name}\n    ${err.message}`);
    process.exitCode = 1;
  }
}

// ---------- util ----------

console.log('util.js');

test('fmtSize 覆盖 B/KB/MB 与空值', () => {
  assert.equal(fmtSize(0), '0 B');
  assert.equal(fmtSize(1023), '1023 B');
  assert.equal(fmtSize(1024), '1.0 KB');
  assert.equal(fmtSize(1536), '1.5 KB');
  assert.equal(fmtSize(1024 * 1024), '1.00 MB');
  assert.equal(fmtSize(1024 * 1024 * 3.5), '3.50 MB');
  assert.equal(fmtSize(null), '—');
  assert.equal(fmtSize(undefined), '—');
});

test('esc 转义 HTML 危险字符', () => {
  assert.equal(esc('<img>&"\''), '&lt;img&gt;&amp;&quot;&#39;');
  assert.equal(esc('正常.png'), '正常.png');
});

test('baseName / dirName 路径处理', () => {
  assert.equal(baseName('/a/b/c.png'), 'c.png');
  assert.equal(baseName('c.png'), 'c.png');
  assert.equal(dirName('/a/b/c.png'), '/a/b');
  assert.equal(dirName('c.png'), '');
});

test('ratioText 压缩率文案', () => {
  assert.equal(ratioText(0.719), '↓ 71.9%');
  assert.equal(ratioText(0), '—');
});

// ---------- ipc：fake internals ----------

function fakeInternals() {
  const callbacks = new Map();
  const invocations = [];
  let nextId = 1;
  return {
    invocations,
    callbacks,
    transformCallback(fn) {
      const id = nextId++;
      callbacks.set(id, fn);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    runCallback(id, payload) {
      callbacks.get(id)?.(payload);
    },
    async invoke(cmd, args) {
      invocations.push({ cmd, args });
      return args?.__ret;
    },
  };
}

console.log('ipc.js — Channel');

test('Channel 按序投递', () => {
  const internals = fakeInternals();
  const got = [];
  const ch = makeChannel(internals, (m) => got.push(m));
  const id = Number(JSON.parse(JSON.stringify(ch)).match(/__CHANNEL__:(\d+)/)[1]);
  internals.runCallback(id, { message: 'a', index: 0 });
  internals.runCallback(id, { message: 'b', index: 1 });
  assert.deepEqual(got, ['a', 'b']);
});

test('Channel 乱序到达时缓冲重排', () => {
  const internals = fakeInternals();
  const got = [];
  const ch = makeChannel(internals, (m) => got.push(m));
  const id = Number(JSON.parse(JSON.stringify(ch)).match(/__CHANNEL__:(\d+)/)[1]);
  internals.runCallback(id, { message: 'c', index: 2 });
  internals.runCallback(id, { message: 'a', index: 0 });
  assert.deepEqual(got, ['a']); // 2 仍在缓冲
  internals.runCallback(id, { message: 'b', index: 1 });
  assert.deepEqual(got, ['a', 'b', 'c']); // 2 被补投
});

test('Channel end 消息清理回调', () => {
  const internals = fakeInternals();
  const got = [];
  const ch = makeChannel(internals, (m) => got.push(m));
  const id = Number(JSON.parse(JSON.stringify(ch)).match(/__CHANNEL__:(\d+)/)[1]);
  internals.runCallback(id, { message: 'a', index: 0 });
  internals.runCallback(id, { end: true, index: 1 });
  assert.equal(internals.callbacks.has(id), false);
  internals.runCallback(id, { message: 'b', index: 1 });
  assert.deepEqual(got, ['a']); // 清理后不再投递
});

test('invoke 透传命令与参数', async () => {
  const internals = fakeInternals();
  const ret = await invoke(internals, 'get_prefs', { __ret: { outputFormat: 'webp' } });
  assert.deepEqual(ret, { outputFormat: 'webp' });
  assert.equal(internals.invocations[0].cmd, 'get_prefs');
});

console.log('ipc.js — onTauriEvent');

await (async () => {
  const internals = fakeInternals();
  const seen = [];
  const unlisten = await onTauriEvent(internals, 'tauri://drag-drop', (evt) => seen.push(evt));
  // 注册形状：plugin:event|listen + {kind:'Any'} target
  const reg = internals.invocations.find((i) => i.cmd === 'plugin:event|listen');
  assert.ok(reg, '应注册事件监听');
  assert.equal(reg.args.event, 'tauri://drag-drop');
  assert.deepEqual(reg.args.target, { kind: 'Any' });
  // 回调投递
  const cbId = reg.args.handler;
  internals.runCallback(cbId, { event: 'tauri://drag-drop', payload: { type: 'drop', paths: ['/x/a.png'] } });
  assert.equal(seen.length, 1);
  assert.deepEqual(seen[0].payload.paths, ['/x/a.png']);
  // 取消监听
  internals.invoke = async () => undefined;
  await unlisten();
  console.log('  ✓ onTauriEvent 注册 / 投递 / 取消');
  passed += 1;
})().catch((err) => {
  console.error(`  ✗ onTauriEvent: ${err.message}`);
  process.exitCode = 1;
});

// ---------- i18n ----------

console.log('i18n.js');

test('detectLocale 按优先级取首个可归一化语言', () => {
  assert.equal(detectLocale(['zh-CN', 'en']), 'zh-CN');
  assert.equal(detectLocale(['en-US']), 'en');
  assert.equal(detectLocale(['ja-JP']), 'ja');
  assert.equal(detectLocale(['zh-Hant-TW']), 'zh-TW');
  assert.equal(detectLocale(['ar-SA', 'fr']), 'fr'); // 首个不可归一化，跳过
  assert.equal(detectLocale([]), 'zh-CN');
  assert.equal(detectLocale('de'), 'de'); // 单个字符串亦可
  assert.equal(detectLocale(['xx-YY']), 'zh-CN');
  assert.equal(detectLocale(null), 'zh-CN');
});

test('normalizeLocale 归一支集与大小写', () => {
  assert.equal(normalizeLocale('zh-Hans-CN'), 'zh-CN');
  assert.equal(normalizeLocale('ZH-tw'), 'zh-TW'); // 大小写不敏感
  assert.equal(normalizeLocale('zh'), 'zh-CN');
  assert.equal(normalizeLocale('zh-SG'), 'zh-CN');
  assert.equal(normalizeLocale('zh-Hant-HK'), 'zh-TW');
  assert.equal(normalizeLocale('pt-BR'), 'pt');
  assert.equal(normalizeLocale('en-US'), 'en');
  assert.equal(normalizeLocale('it'), null); // 不支持的语言
  assert.equal(normalizeLocale(''), null);
  assert.equal(normalizeLocale(null), null);
});

test('t() 插值、缺 key 回落与未知语言回落', () => {
  for (const { code } of LOCALES) {
    const tr = createTranslator(code);
    assert.ok(tr.t('list.count', { n: 5 }).includes('5'), `${code} 的 list.count 应含 5`);
  }
  // 缺 key → 回落中文；再缺 → key 本身
  assert.equal(createTranslator('en').t('no.such.key'), 'no.such.key');
  // 未知语言整体回落 zh-CN 字典
  const xx = createTranslator('xx');
  assert.equal(xx.t('btn.run'), DICTS[FALLBACK_LOCALE]['btn.run']);
  assert.equal(xx.t('btn.run'), '开始压缩');
  // 插值变量缺失时保留占位符
  assert.equal(createTranslator('zh-CN').t('list.count'), '{n} 个文件');
});

test('字典 key 全集各语言一致（防漏翻）', () => {
  const base = Object.keys(DICTS[FALLBACK_LOCALE]).sort();
  for (const { code } of LOCALES) {
    assert.deepEqual(Object.keys(DICTS[code]).sort(), base, `${code} 的 key 集合应与 zh-CN 一致`);
  }
});

test('THEME_IDS 恰好六套且非空', () => {
  assert.equal(THEME_IDS.length, 6);
  for (const id of ['teal', 'dark', 'violet', 'ocean', 'rose', 'forest']) {
    assert.ok(THEME_IDS.includes(id), `应包含主题 ${id}`);
  }
  for (const id of THEME_IDS) {
    assert.ok(typeof id === 'string' && id.length > 0, `主题 id 应为非空字符串: ${id}`);
  }
});

test('主题菜单与压缩程度滑条的 i18n key 全覆盖', () => {
  // 新 key：顶栏主题菜单 + 压缩程度滑条（含左右端点标签）
  const newKeys = ['menu.theme', 'field.level', 'field.levelHint', 'level.strong', 'level.fine'];
  for (const { code } of LOCALES) {
    for (const key of newKeys) {
      const v = DICTS[code][key];
      assert.ok(typeof v === 'string' && v.length > 0, `${code} 的 ${key} 应为非空译文`);
    }
    // 端点标签不得相同（否则用户分不清方向）
    assert.notEqual(DICTS[code]['level.strong'], DICTS[code]['level.fine'], `${code} 的 level 端点标签应可区分`);
  }
  // 旧侧栏主题字段 key 已随功能迁移移除
  for (const { code } of LOCALES) {
    assert.ok(!('field.theme' in DICTS[code]), `${code} 不应再保留已迁移的 field.theme`);
    assert.ok(!('field.themeHint' in DICTS[code]), `${code} 不应再保留已迁移的 field.themeHint`);
  }
  // 英文译文抽验（防机翻/串台）
  assert.equal(DICTS.en['menu.theme'], 'Theme');
  assert.equal(DICTS.en['field.level'], 'Compression level');
  assert.equal(DICTS.en['level.strong'], 'Aggressive');
  assert.equal(DICTS.en['level.fine'], 'Fine');
});

test('LOCALES 至少 10 种、code 唯一且均有字典', () => {
  assert.ok(LOCALES.length >= 10, '至少 10 种语言');
  const codes = new Set();
  for (const l of LOCALES) {
    assert.ok(l.code && l.native, 'locale 需含 code 与 native');
    assert.ok(!codes.has(l.code), `code 重复: ${l.code}`);
    codes.add(l.code);
    assert.ok(DICTS[l.code], `${l.code} 缺少字典`);
  }
  assert.equal(LOCALES[0].code, 'zh-CN', 'zh-CN 应为默认首选');
});

console.log(`\n前端逻辑测试：${passed} 项通过${process.exitCode ? '（存在失败）' : ''}`);
