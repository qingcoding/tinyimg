//! TinyImg — 库入口
//!
//! 压缩核心与 CLI 分离：核心能力（量化 / PNG 写出 / 压缩管线）以库形式暴露，
//! 便于后续 GUI（Tauri）阶段直接复用，也为集成测试提供入口。

pub mod cli;
pub mod compress;
pub mod decode;
pub mod pngw;
pub mod quant;
pub mod util;
