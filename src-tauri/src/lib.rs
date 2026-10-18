//! TinyImg 桌面端（Tauri 2）库入口
//!
//! 压缩能力全部来自 `tinyimg` 库（与 CLI 同一套核心），本 crate 只做
//! 桌面壳：原生对话框、批量压缩 IPC、偏好持久化。

pub mod commands;
pub mod prefs;
