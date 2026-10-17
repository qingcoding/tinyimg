// 图小小 TinyImg — 前端逻辑
//
// 模块划分：
// - util.js：纯函数（体积格式化 / 转义 / 路径处理）
// - ipc.js ：Tauri IPC 层（invoke / Channel / 事件监听）
// - i18n.js：多语言字典、语言探测与翻译器（无 DOM 依赖）
// - app.js ：状态、渲染与交互编排（本文件）

import { fmtSize, esc, baseName, dirName, ratioText } from './util.js';
import { makeChannel, onTauriEvent, invoke } from './ipc.js';
import { LOCALES, FALLBACK_LOCALE, THEME_IDS, detectLocale, createTranslator } from './i18n.js';

const internals = window.__TAURI_INTERNALS__;

// 主题色块的内联渐变（accent / accent-2 十六进制值）。
// 与 styles.css 中各 [data-theme] 块的 --accent / --accent-2 保持一致：
// 色块需在切换前直接画出目标主题的渐变，无法引用当前主题的 CSS 变量。
const THEME_ACCENTS = {
  teal: ['#0d9488', '#0891b2'],
  dark: ['#22d3ee', '#0ea5e9'],
  violet: ['#7c3aed', '#a855f7'],
  ocean: ['#2563eb', '#0ea5e9'],
  rose: ['#e11d48', '#f43f5e'],
  forest: ['#059669', '#10b981'],
};

// ---------------------------------------------------------------- 状态

const state = {
  /** path -> { path, name, dir, size, result } */
  entries: new Map(),
  prefs: {
    lastInputDir: null,
    lastOutputDir: null,
    outputFormat: 'webp',
    language: null,
    theme: 'teal',
  },
  busy: false,
};

// ---------------------------------------------------------------- 多语言

let translator = createTranslator(FALLBACK_LOCALE);
const t = (key, vars) => translator.t(key, vars);

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
const themeRow = $('themeRow');
const btnSettings = $('btnSettings');
const settingsMenu = $('settingsMenu');
const langMenuList = $('langMenuList');
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
      <td class="col-status" data-cell="status"><span class="badge badge-wait">${t('status.wait')}</span></td>
      <td class="col-actions">
        <button class="row-icon" type="button" data-act="reveal" title="${esc(t('action.reveal'))}">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
        </button>
        <button class="row-icon" type="button" data-act="copy" title="${esc(t('action.copy'))}">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
        </button>
      </td>`;
    fileTbody.appendChild(tr);
    rowEls.set(e.path, tr);
    if (e.result) applyResultToRow(e);
  }
  listCount.textContent = t('list.count', { n: entries.length });
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
    status.innerHTML = `<span class="badge badge-done">${t('status.done')}</span>`;
    tr.title = `${entry.path}\n${r.detail}${r.output ? `\n输出: ${r.output}` : ''}`;
  } else if (r.status === 'skipped') {
    comp.textContent = '—';
    ratio.innerHTML = '<span class="ratio-flat">—</span>';
    status.innerHTML = `<span class="badge badge-skip">${t('status.skip')}</span>`;
    tr.title = `${entry.path}\n${r.detail}`;
  } else {
    comp.textContent = '—';
    ratio.innerHTML = '<span class="ratio-flat">—</span>';
    status.innerHTML = `<span class="badge badge-fail">${t('status.fail')}</span>`;
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
  bottomInfo.textContent = t('list.count', { n: entries.length });
}

function updateButtons() {
  btnRun.disabled = state.busy || state.entries.size === 0;
  btnClear.disabled = state.busy || state.entries.size === 0;
  btnRun.innerHTML = state.busy
    ? '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M12 3a9 9 0 1 0 9 9" /></svg>'
      + esc(t('btn.running'))
    : '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5.5v13l11-6.5z"/></svg>'
      + esc(t('btn.run'));
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

/** 渲染输出目录栏：已选则显示路径，否则显示本地化占位（语言切换时随之刷新） */
function renderOutputDir() {
  if (state.prefs.lastOutputDir) {
    outputDirPath.textContent = state.prefs.lastOutputDir;
    outputDirPath.classList.add('set');
    outputDirPath.title = state.prefs.lastOutputDir;
  } else {
    outputDirPath.textContent = t('dir.notSet');
    outputDirPath.classList.remove('set');
    outputDirPath.title = '';
  }
}

function applyPrefs() {
  // 输出格式
  formatSeg.querySelectorAll('.seg').forEach((b) =>
    b.classList.toggle('active', b.dataset.format === state.prefs.outputFormat)
  );
  // 输出目录展示
  renderOutputDir();
  // applyI18n 内部会重建语言菜单勾选态与主题色块
  applyI18n();
  applyTheme();
}

// ---------------------------------------------------------------- 顶部设置菜单

/** 打开 / 关闭设置菜单（语言项） */
function toggleSettingsMenu() {
  settingsMenu.hidden ? openSettingsMenu() : closeSettingsMenu();
}

function openSettingsMenu() {
  settingsMenu.hidden = false;
  btnSettings.setAttribute('aria-expanded', 'true');
}

function closeSettingsMenu() {
  settingsMenu.hidden = true;
  btnSettings.setAttribute('aria-expanded', 'false');
}

// ---------------------------------------------------------------- 语言与主题

/** 填充顶部设置菜单的语言项（母语名 + 当前项勾选），每次语言切换后重建 */
function buildLangMenu() {
  langMenuList.innerHTML = '';
  for (const { code, native } of LOCALES) {
    const item = document.createElement('button');
    item.type = 'button';
    item.className = 'menu-item';
    item.dataset.code = code;
    item.setAttribute('role', 'menuitem');
    if (code === translator.locale) {
      item.classList.add('active');
      item.setAttribute('aria-current', 'true');
    }
    const label = document.createElement('span');
    label.className = 'menu-item-label';
    label.textContent = native;
    const check = document.createElement('span');
    check.className = 'menu-item-check';
    check.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
    item.append(label, check);
    item.addEventListener('click', () => {
      applyLocale(code, { persist: true });
      closeSettingsMenu();
    });
    langMenuList.appendChild(item);
  }
}

/** 生成主题色块（内联该主题 accent 双色渐变小圆点 + 主题名） */
function buildThemeChips() {
  themeRow.innerHTML = '';
  for (const id of THEME_IDS) {
    const [a1, a2] = THEME_ACCENTS[id] || ['#0d9488', '#0891b2'];
    const chip = document.createElement('button');
    chip.type = 'button';
    chip.className = 'theme-chip';
    chip.dataset.themeId = id;
    chip.title = t(`theme.${id}`);
    const dot = document.createElement('span');
    dot.className = 'theme-chip-dot';
    dot.style.background = `linear-gradient(135deg, ${a1}, ${a2})`;
    const label = document.createElement('span');
    label.textContent = t(`theme.${id}`);
    chip.append(dot, label);
    themeRow.appendChild(chip);
  }
}

function syncThemeChips() {
  themeRow.querySelectorAll('.theme-chip').forEach((chip) =>
    chip.classList.toggle('active', chip.dataset.themeId === state.prefs.theme)
  );
}

/** 应用静态文案（data-i18n / data-i18n-title）、html lang、标题与控件选中态 */
function applyI18n() {
  document.querySelectorAll('[data-i18n]').forEach((el) => {
    el.textContent = t(el.dataset.i18n);
  });
  document.querySelectorAll('[data-i18n-title]').forEach((el) => {
    el.title = t(el.dataset.i18nTitle);
  });
  document.documentElement.lang = translator.locale;
  document.title = t('app.title');
  buildThemeChips();
  syncThemeChips();
  buildLangMenu();
}

/** 应用主题（data-theme 驱动 CSS 变量整块切换） */
function applyTheme() {
  const theme = THEME_IDS.includes(state.prefs.theme) ? state.prefs.theme : 'teal';
  state.prefs.theme = theme;
  document.documentElement.dataset.theme = theme;
}

/**
 * 切换语言：重建翻译器 → 刷新静态文案 → 重渲染动态区域。
 * @param {string} code 目标语言 code（非法值回落中文）
 * @param {{ persist?: boolean }} [options] persist=true 时写回偏好
 */
function applyLocale(code, { persist = false } = {}) {
  const locale = LOCALES.some((l) => l.code === code) ? code : FALLBACK_LOCALE;
  translator = createTranslator(locale);
  if (persist) {
    state.prefs.language = locale;
    savePrefs();
  }
  applyI18n();
  renderOutputDir();
  rerenderAll();
}

/** 语言切换后重渲染所有动态区域 */
function rerenderAll() {
  renderList();
  renderStats();
  updateButtons();
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
    showToast(t('toast.batchDone', { done, skipped, failed, saved: fmtSize(saved) }));
  } catch (err) {
    showToast(t('toast.batchFail', { err: err?.message || err }));
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
  if (picked.length > 0 && added === 0) showToast(t('toast.dupFiles'));
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
  if (entries.length === 0) showToast(t('toast.folderEmpty'));
  else if (added === 0) showToast(t('toast.folderDup'));
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

btnSettings.addEventListener('click', (ev) => {
  ev.stopPropagation();
  toggleSettingsMenu();
});

// 点击菜单外部 / 按 Esc 关闭设置菜单
document.addEventListener('click', (ev) => {
  if (settingsMenu.hidden) return;
  if (!ev.target.closest('.menu-wrap')) closeSettingsMenu();
});
document.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape' && !settingsMenu.hidden) closeSettingsMenu();
});

themeRow.addEventListener('click', (ev) => {
  const chip = ev.target.closest('.theme-chip');
  if (!chip) return;
  const id = chip.dataset.themeId;
  if (!THEME_IDS.includes(id)) return;
  state.prefs.theme = id;
  savePrefs();
  applyTheme();
  applyI18n();
});

btnRun.addEventListener('click', runCompress);

// 行尾操作图标（事件委托）：done 行操作压缩产物，其余行操作原文件
fileTbody.addEventListener('click', (ev) => {
  const btn = ev.target.closest('.row-icon');
  if (!btn) return;
  const tr = btn.closest('tr');
  const entry = tr?.dataset.path ? state.entries.get(tr.dataset.path) : null;
  if (!entry) return;
  const target =
    entry.result?.status === 'done' && entry.result.output
      ? entry.result.output
      : entry.path;
  const act = btn.dataset.act;
  if (act !== 'reveal' && act !== 'copy') return;
  invoke(internals, act === 'reveal' ? 'reveal_in_finder' : 'copy_image', { path: target })
    .then(() => showToast(t(act === 'reveal' ? 'toast.revealOk' : 'toast.copyOk')))
    .catch((err) =>
      showToast(t(act === 'reveal' ? 'toast.revealFail' : 'toast.copyFail', { err: err?.message || err }))
    );
});

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
  if (entries.length === 0) showToast(t('toast.dropEmpty'));
  else if (added === 0) showToast(t('toast.dropDup'));
});

// ---------------------------------------------------------------- 启动

(async function init() {
  // 先渲染主题控件，避免首闪空控件
  buildThemeChips();
  try {
    const prefs = await invoke(internals, 'get_prefs');
    if (prefs) state.prefs = { ...state.prefs, ...prefs };
  } catch {
    /* 首次启动或 IPC 异常时用默认值 */
  }
  // 主题非法值回落 teal
  if (!THEME_IDS.includes(state.prefs.theme)) state.prefs.theme = 'teal';
  // 语言：优先用户手动选择；否则跟随系统（检测结果不写回 prefs，保持跟随）
  const lang =
    state.prefs.language || detectLocale([navigator.language, ...(navigator.languages || [])]);
  applyLocale(lang, { persist: false });
  applyPrefs();
  renderList();
  renderStats();
  updateButtons();
})();
