// 前端逻辑单测（Node 直接运行，无测试框架）：
//   node tests/ui_logic.test.mjs
// 覆盖 util.js 纯函数与 ipc.js 的 Channel 有序投递 / end 清理 / 事件监听。
// app.js 依赖 DOM 不在本测试范围（由应用运行期验证）。

import assert from 'node:assert/strict';
import { fmtSize, esc, baseName, dirName, ratioText } from '../ui/util.js';
import { makeChannel, onTauriEvent, invoke } from '../ui/ipc.js';

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

console.log(`\n前端逻辑测试：${passed} 项通过${process.exitCode ? '（存在失败）' : ''}`);
