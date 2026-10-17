// 纯工具函数（无 DOM / IPC 依赖，可单测）

export const fmtSize = (bytes) => {
  if (bytes === null || bytes === undefined) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
};

export const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

export const baseName = (p) => String(p).split('/').pop() || String(p);

export const dirName = (p) => {
  const i = String(p).lastIndexOf('/');
  return i > 0 ? String(p).slice(0, i) : '';
};

/** 压缩率文案：done 用 ↓ 百分比，其余用占位 */
export const ratioText = (ratio) =>
  ratio > 0 ? `↓ ${(ratio * 100).toFixed(1)}%` : '—';
