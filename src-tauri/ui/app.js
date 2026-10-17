// 图小小 TinyImg — 前端逻辑
//
// 模块划分：
// - util.js：纯函数（体积格式化 / 转义 / 路径处理）
// - ipc.js ：Tauri IPC 层（invoke / Channel / 事件监听）
// - app.js ：状态、渲染与交互编排（本文件）

import { fmtSize, esc, baseName, dirName, ratioText } from './util.js';
import { makeChannel, onTauriEvent, invoke } from './ipc.js';

const internals = window.__TAURI_INTERNALS__;

// ---------------------------------------------------------------- 状态

const state = {
  /** path -> { path, name, dir, size, result } */
  entries: new Map(),
  prefs: { lastInputDir: null, lastOutputDir: null, outputFormat: 'webp' },
  busy: false,
};

// ---------------------------------------------------------------- DOM

const $ = (id) => document.getElementById(id);
const dropzone = $('dropzone');
const listPanel = $('listPanel');
const fileTbody = $('fileTbody');
const listCount = $('listCount');
const btnPickFiles = $('btnPickFiles');
const btnPickFolder = $('btnPickFolder');
const btnPickOutput = $('btnPickOutput');
const btnRun = $('btnRun');
const btnClear = $('btnClear');
const outputDirPath = $('outputDirPath');
const formatSeg = $('formatSeg');
const statSaved = $('statSaved');
const statRatio = $('statRatio');
const statDone = $('statDone');
const progressBar = $('progressBar');
const bottomInfo = $('bottomInfo');
const toast = $('toast');

const rowEls = new Map();
let toastTimer = null;

// ---------------------------------------------------------------- 渲染

function renderList() {
  rowEls.clear();
  fileTbody.innerHTML = '';
  const entries = [...state.entries.values()];
  if (entries.length === 0) {
    listPanel.hidden = true;
    return;
  }
  listPanel.hidden = false;
  for (const e of entries) {
    const tr = document.createElement('tr');
    tr.dataset.path = e.path;
    tr.title = e.path;
    tr.innerHTML = `
      <td class="col-name">
        <span class="file-name">${esc(e.name)}</span>
        <span class="file-dir">${esc(e.dir)}</span>
      </td>
      <td class="col-size">${fmtSize(e.size)}</td>
      <td class="col-size" data-cell="comp">—</td>
      <td class="col-ratio" data-cell="ratio">—</td>
      <td class="col-status" data-cell="status"><span class="badge badge-wait">等待</span></td>`;
    fileTbody.appendChild(tr);
    rowEls.set(e.path, tr);
    if (e.result) applyResultToRow(e);
  }
  listCount.textContent = `${entries.length} 个文件`;
}

function applyResultToRow(entry) {
  const r = entry.result;
  const tr = rowEls.get(entry.path);
  if (!tr) return;
  const comp = tr.querySelector('[data-cell="comp"]');
  const ratio = tr.querySelector('[data-cell="ratio"]');
  const status = tr.querySelector('[data-cell="status"]');
  if (r.status === 'done') {
    comp.textContent = fmtSize(r.compressed);
    ratio.innerHTML = `<span class="ratio-good">${ratioText(r.ratio)}</span>`;
    status.innerHTML = '<span class="badge badge-done">完成</span>';
    tr.title = `${entry.path}\n${r.detail}${r.output ? `\n输出: ${r.output}` : ''}`;
  } else if (r.status === 'skipped') {
    comp.textContent = '—';
    ratio.innerHTML = '<span class="ratio-flat">—</span>';
    status.innerHTML = '<span class="badge badge-skip">跳过</span>';
    tr.title = `${entry.path}\n${r.detail}`;
  } else {
    comp.textContent = '—';
    ratio.innerHTML = '<span class="ratio-flat">—</span>';
    status.innerHTML = '<span class="badge badge-fail">失败</span>';
    tr.title = `${entry.path}\n${r.detail}`;
  }
}

function renderStats() {
  const entries = [...state.entries.values()];
  const done = entries.filter((e) => e.result?.status === 'done');
  const saved = done.reduce((s, e) => s + (e.result.original - e.result.compressed), 0);
  const doneOriginal = done.reduce((s, e) => s + e.result.original, 0);

  statSaved.textContent = saved > 0 ? fmtSize(saved) : '0 B';
  statRatio.textContent = doneOriginal > 0 ? ratioText(saved / doneOriginal) : '—';
  statRatio.classList.toggle('good', doneOriginal > 0);
  statDone.textContent = `${done.length} / ${entries.length}`;
  progressBar.style.width = entries.length ? `${(done.length / entries.length) * 100}%` : '0%';
  bottomInfo.textContent = `共 ${entries.length} 个文件`;
}

function updateButtons() {
  btnRun.disabled = state.busy || state.entries.size === 0;
  btnClear.disabled = state.busy || state.entries.size === 0;
  btnRun.innerHTML = state.busy
    ? '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M12 3a9 9 0 1 0 9 9" /></svg>压缩中…'
    : '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5.5v13l11-6.5z"/></svg>开始压缩';
}

function showToast(text) {
  toast.textContent = text;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { toast.hidden = true; }, 3600);
}

// ---------------------------------------------------------------- 偏好

function savePrefs() {
  invoke(internals, 'save_prefs', { prefs: state.prefs }).catch(() => {});
}

function applyPrefs() {
  // 输出格式
  formatSeg.querySelectorAll('.seg').forEach((b) =>
    b.classList.toggle('active', b.dataset.format === state.prefs.outputFormat)
  );
  // 输出目录展示
  if (state.prefs.lastOutputDir) {
    outputDirPath.textContent = state.prefs.lastOutputDir;
    outputDirPath.classList.add('set');
    outputDirPath.title = state.prefs.lastOutputDir;
  } else {
    outputDirPath.textContent = '未选择';
    outputDirPath.classList.remove('set');
    outputDirPath.title = '';
  }
}

// ---------------------------------------------------------------- 文件管理

function addEntries(entries) {
  let added = 0;
  for (const e of entries) {
    if (state.entries.has(e.path)) continue;
    state.entries.set(e.path, {
      path: e.path,
      name: e.name || baseName(e.path),
      dir: dirName(e.path),
      size: e.size || 0,
      result: null,
    });
    added += 1;
  }
  if (added > 0) {
    // 按文件名排序，保持列表稳定
    const sorted = [...state.entries.values()].sort((a, b) =>
      a.name.localeCompare(b.name, 'zh-Hans-CN')
    );
    state.entries = new Map(sorted.map((e) => [e.path, e]));
    renderList();
    renderStats();
    updateButtons();
  }
  return added;
}

// ---------------------------------------------------------------- 压缩

function applyResult(r) {
  const entry = state.entries.get(r.input);
  if (!entry) return;
  entry.result = r;
  applyResultToRow(entry);
  renderStats();
}

async function runCompress() {
  if (state.busy || state.entries.size === 0) return;
  state.busy = true;
  updateButtons();

  const paths = [...state.entries.keys()];
  const channel = makeChannel(internals, applyResult);

  try {
    const results = await invoke(internals, 'compress_batch', {
      paths,
      outputDir: state.prefs.lastOutputDir || null,
      format: state.prefs.outputFormat,
      onProgress: channel,
    });
    // Channel 已流式更新，返回值做兜底对齐（极端丢包场景）
    for (const r of results) applyResult(r);

    const done = results.filter((r) => r.status === 'done').length;
    const skipped = results.filter((r) => r.status === 'skipped').length;
    const failed = results.filter((r) => r.status === 'failed').length;
    const saved = results
      .filter((r) => r.status === 'done')
      .reduce((s, r) => s + (r.original - r.compressed), 0);
    showToast(`压缩完成 · 成功 ${done} · 跳过 ${skipped} · 失败 ${failed} · 共节省 ${fmtSize(saved)}`);
  } catch (err) {
    showToast(`压缩失败：${err?.message || err}`);
  } finally {
    state.busy = false;
    updateButtons();
  }
}

// ---------------------------------------------------------------- 事件绑定

btnPickFiles.addEventListener('click', async () => {
  const picked = await invoke(internals, 'pick_files', {
    startDir: state.prefs.lastInputDir,
  }).catch(() => []);
  const added = addEntries(picked);
  if (picked.length > 0) {
    const dir = dirName(picked[0].path);
    if (dir) {
      state.prefs.lastInputDir = dir;
      savePrefs();
    }
  }
  if (picked.length > 0 && added === 0) showToast('所选文件已在列表中');
});

btnPickFolder.addEventListener('click', async () => {
  const dir = await invoke(internals, 'pick_folder', {
    startDir: state.prefs.lastInputDir,
  }).catch(() => null);
  if (!dir) return;
  const entries = await invoke(internals, 'collect_paths', { paths: [dir] }).catch(() => []);
  const added = addEntries(entries);
  state.prefs.lastInputDir = dir;
  savePrefs();
  if (entries.length === 0) showToast('该文件夹中没有找到 PNG / JPEG 图片');
  else if (added === 0) showToast('文件夹中的图片都已在列表中');
});

btnPickOutput.addEventListener('click', async () => {
  const dir = await invoke(internals, 'pick_output_dir', {
    startDir: state.prefs.lastOutputDir,
  }).catch(() => null);
  if (!dir) return;
  state.prefs.lastOutputDir = dir;
  savePrefs();
  applyPrefs();
});

formatSeg.addEventListener('click', (ev) => {
  const btn = ev.target.closest('.seg');
  if (!btn || state.busy) return;
  state.prefs.outputFormat = btn.dataset.format;
  savePrefs();
  applyPrefs();
});

btnRun.addEventListener('click', runCompress);

btnClear.addEventListener('click', () => {
  if (state.busy) return;
  state.entries = new Map();
  renderList();
  renderStats();
  updateButtons();
});

// 系统拖拽（tauri://drag-*，payload: { type, paths }）
onTauriEvent(internals, 'tauri://drag-enter', (evt) => {
  if (evt?.payload?.type) dropzone.classList.add('dragover');
});
onTauriEvent(internals, 'tauri://drag-over', (evt) => {
  if (evt?.payload?.type) dropzone.classList.add('dragover');
});
onTauriEvent(internals, 'tauri://drag-leave', () => dropzone.classList.remove('dragover'));
onTauriEvent(internals, 'tauri://drag-drop', async (evt) => {
  dropzone.classList.remove('dragover');
  const paths = evt?.payload?.paths;
  if (!Array.isArray(paths) || paths.length === 0) return;
  const entries = await invoke(internals, 'collect_paths', { paths }).catch(() => []);
  const added = addEntries(entries);
  if (entries.length === 0) showToast('拖入的内容中没有可压缩的图片');
  else if (added === 0) showToast('拖入的图片都已在列表中');
});

// ---------------------------------------------------------------- 启动

(async function init() {
  try {
    const prefs = await invoke(internals, 'get_prefs');
    if (prefs) state.prefs = { ...state.prefs, ...prefs };
  } catch {
    /* 首次启动或 IPC 异常时用默认值 */
  }
  applyPrefs();
  renderList();
  renderStats();
  updateButtons();
})();
