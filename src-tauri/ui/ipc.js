// IPC 层：直连 Tauri 注入的 __TAURI_INTERNALS__（无 npm 依赖）
//
// - invoke：命令调用（参数按 Tauri v2 约定用 camelCase）
// - Channel：Rust → 前端的流式推送，与官方 @tauri-apps/api 语义一致
//   （按 index 重排缓冲、end 消息清理回调）
// - onTauriEvent：监听 Tauri 系统事件（拖拽 tauri://drag-*）

/**
 * 构造一个可序列化为 "__CHANNEL__:<id>" 的 Channel 对象。
 * @param internals window.__TAURI_INTERNALS__
 * @param onmessage 收到消息的回调（按发送顺序）
 */
export function makeChannel(internals, onmessage) {
  let next = 0;
  const buffer = new Map();
  const id = internals.transformCallback((evt) => {
    if (evt && typeof evt === 'object' && 'end' in evt) {
      internals.unregisterCallback(id);
      return;
    }
    const index = evt?.index ?? 0;
    const message = evt?.message;
    if (index === next) {
      onmessage(message);
      next += 1;
      while (buffer.has(next)) {
        onmessage(buffer.get(next));
        buffer.delete(next);
        next += 1;
      }
    } else if (index > next) {
      buffer.set(index, message);
    }
  });
  return { toJSON: () => `__CHANNEL__:${id}` };
}

/** 监听 Tauri 事件，返回取消监听函数（失败时退化为 noop，不影响主流程） */
export function onTauriEvent(internals, eventName, handler) {
  const noop = () => {};
  try {
    const cbId = internals.transformCallback((evt) => handler(evt));
    return internals
      .invoke('plugin:event|listen', { event: eventName, target: { kind: 'Any' }, handler: cbId })
      .then((eventId) => () =>
        internals.invoke('plugin:event|unlisten', { event: eventName, eventId }).catch(noop)
      )
      .catch(() => noop);
  } catch {
    return Promise.resolve(noop);
  }
}

/** 命令调用包装 */
export const invoke = (internals, cmd, args = {}) => internals.invoke(cmd, args);
