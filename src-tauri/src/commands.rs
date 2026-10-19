//! IPC 命令层：前端 ↔ 压缩核心的桥。
//!
//! 命令清单：
//! - `pick_files` / `pick_folder` / `pick_output_dir`：原生对话框（带上次目录作默认位置）
//! - `collect_paths`：展开目录为待压缩文件列表（复用库的 collect_files）
//! - `compress_batch`：并行批量压缩，逐文件经 Channel 推送进度
//! - `get_prefs` / `save_prefs`：上次打开位置与输出格式的记忆
//! - `reveal_in_finder`：在系统文件管理器中揭示文件（macOS Finder 选中 / Windows 资源管理器 / Linux）
//! - `copy_image`：复制图片到系统剪贴板（解码为 RGBA 位图，粘贴即原图）
//!
//! 线程模型：`reveal_in_finder` / `copy_image` 均为**同步命令**（Tauri 同步命令在主线程执行），
//! 因为 macOS 的 NSPasteboard（arboard）与 Finder 揭示都要求主线程调用。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Window};
use tauri_plugin_dialog::{DialogExt, FilePath};

use tinyimg::compress::{self, Options, OutputFormat, Outcome};

use crate::prefs::{self, Prefs};

/// 待压缩文件条目（选择器 / 目录展开的返回单元）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub size: u64,
}

/// 单文件压缩结果（经 Channel 流式推送给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub name: String,
    pub input: String,
    pub output: Option<String>,
    pub original: u64,
    pub compressed: u64,
    /// 节省比例 0..=1
    pub ratio: f64,
    /// done / skipped / failed
    pub status: String,
    pub detail: String,
    pub width: u32,
    pub height: u32,
    pub out_format: String,
    /// 单文件处理耗时（毫秒），前端用于"处理时间"列展示
    pub elapsed_ms: u64,
}

fn fp_to_string(fp: &FilePath) -> Option<String> {
    fp.as_path().map(|p| p.to_string_lossy().to_string())
}

/// 对话框默认目录：仅当上次记录的目录仍然存在时才使用
fn valid_start_dir(dir: &Option<String>) -> Option<String> {
    dir.as_ref()
        .filter(|d| Path::new(d).is_dir())
        .map(|d| d.to_string())
}

// ---------------------------------------------------------------- 选择器

/// 选择单个或多个图片文件（原生对话框，多选）
#[tauri::command]
pub async fn pick_files(window: Window, start_dir: Option<String>) -> Vec<FileEntry> {
    let app = window.app_handle().clone();
    let start_dir = valid_start_dir(&start_dir);
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app
            .dialog()
            .file()
            .add_filter("图片文件", &["png", "jpg", "jpeg"])
            .set_title("选择要压缩的图片");
        if let Some(dir) = start_dir {
            builder = builder.set_directory(dir);
        }
        if let Some(win) = app.get_webview_window("main") {
            builder = builder.set_parent(&win);
        }
        builder.blocking_pick_files()
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    picked
        .iter()
        .filter_map(|fp| fp_to_string(fp))
        .filter_map(|p| file_entry(&PathBuf::from(p)))
        .collect()
}

/// 选择整个文件夹（返回文件夹路径本身，文件展开由 collect_paths 完成）
#[tauri::command]
pub async fn pick_folder(window: Window, start_dir: Option<String>) -> Option<String> {
    let app = window.app_handle().clone();
    let start_dir = valid_start_dir(&start_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app
            .dialog()
            .file()
            .set_title("选择要压缩的文件夹");
        if let Some(dir) = start_dir {
            builder = builder.set_directory(dir);
        }
        if let Some(win) = app.get_webview_window("main") {
            builder = builder.set_parent(&win);
        }
        builder.blocking_pick_folder()
    })
    .await
    .ok()
    .flatten()
    .and_then(|fp| fp_to_string(&fp))
}

/// 选择输出文件夹
#[tauri::command]
pub async fn pick_output_dir(window: Window, start_dir: Option<String>) -> Option<String> {
    let app = window.app_handle().clone();
    let start_dir = valid_start_dir(&start_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app
            .dialog()
            .file()
            .set_title("选择输出文件夹");
        if let Some(dir) = start_dir {
            builder = builder.set_directory(dir);
        }
        if let Some(win) = app.get_webview_window("main") {
            builder = builder.set_parent(&win);
        }
        builder.blocking_pick_folder()
    })
    .await
    .ok()
    .flatten()
    .and_then(|fp| fp_to_string(&fp))
}

// ---------------------------------------------------------------- 文件收集

fn file_entry(path: &Path) -> Option<FileEntry> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(FileEntry {
        path: path.to_string_lossy().to_string(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        size: meta.len(),
    })
}

/// 展开输入路径（文件直接入列，目录递归扫描），去重 + 排序，只保留支持格式
#[tauri::command]
pub fn collect_paths(paths: Vec<String>) -> Vec<FileEntry> {
    let pathbufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    compress::collect_files(&pathbufs, true)
        .iter()
        .filter_map(|p| file_entry(p))
        .collect()
}

// ---------------------------------------------------------------- 压缩

/// 质量档位：由"压缩程度"滑条值（0-100，默认 100）经 level_params 映射。
/// 100 = 256 色 / q88（与 CLI high 档一致，GUI 默认行为）。
/// 滑条 = 0 时启用极致档（Zopfli 穷举压缩），其余档位走 flate2 快速 zlib。
/// dither 暂不暴露给 GUI（保持默认关闭，误差扩散收益有限且拖慢批量）。
fn default_options(format: &str, level: u8) -> Options {
    let output_format = match format {
        "auto" | "a" | "自动" => OutputFormat::Auto,
        "webp" => OutputFormat::WebP,
        // 无法识别 → Auto（与前端默认一致）
        _ => OutputFormat::Auto,
    };
    let (colors, quality) = compress::level_params(level);
    Options {
        colors,
        quality,
        dither: false,
        output_format,
        extreme: level == 0,
    }
}

/// 单个文件的输出策略：
/// - 输出目录与输入同目录 → 退回库默认 `<原名>-min.<ext>`，绝不覆盖原文件
/// - 输出目录为独立目录 → 覆盖写入（该目录是用户专为工具输出选择的）
fn resolve_output(input: &Path, output_dir: &Option<String>) -> (Option<PathBuf>, bool) {
    match output_dir.as_deref() {
        Some(dir) if Path::new(dir) != input.parent().unwrap_or(Path::new("")) => {
            (Some(PathBuf::from(dir)), true)
        }
        _ => (None, false),
    }
}

fn compress_one(input: &Path, output_dir: &Option<String>, opts: &Options) -> FileResult {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (out_dir, overwrite) = resolve_output(input, output_dir);
    let start = std::time::Instant::now();

    let fail = |status: &str, detail: String, original: u64| FileResult {
        name: name.clone(),
        input: input.to_string_lossy().to_string(),
        output: None,
        original,
        compressed: 0,
        ratio: 0.0,
        status: status.to_string(),
        detail,
        width: 0,
        height: 0,
        out_format: String::new(),
        elapsed_ms: start.elapsed().as_millis() as u64,
    };

    match compress::process_file(input, out_dir.as_deref(), overwrite, opts) {
        Outcome::Done {
            output,
            original,
            compressed,
            detail,
            info,
            ..
        } => FileResult {
            name,
            input: input.to_string_lossy().to_string(),
            output: Some(output.to_string_lossy().to_string()),
            original,
            compressed,
            ratio: if original > 0 {
                (original - compressed) as f64 / original as f64
            } else {
                0.0
            },
            status: "done".to_string(),
            detail,
            width: info.width,
            height: info.height,
            out_format: info.out_format.to_string(),
            elapsed_ms: start.elapsed().as_millis() as u64,
        },
        Outcome::Skipped { reason, .. } => {
            let original = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            fail("skipped", reason, original)
        }
        Outcome::Failed { error, .. } => {
            let original = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            fail("failed", error, original)
        }
    }
}

/// 批量压缩：并行处理，每完成一个文件经 `on_progress` Channel 推送一条结果。
/// `level` 为压缩程度（0-100，前端滑条值；越界在 level_params 内 clamp）。
#[tauri::command]
pub async fn compress_batch(
    paths: Vec<String>,
    output_dir: Option<String>,
    format: String,
    level: u8,
    on_progress: Channel<FileResult>,
) -> Vec<FileResult> {
    let opts = default_options(&format, level);
    let pathbufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();

    tauri::async_runtime::spawn_blocking(move || {
        let files = compress::collect_files(&pathbufs, true);
        use rayon::prelude::*;
        files
            .par_iter()
            .map(|f| {
                let result = compress_one(f, &output_dir, &opts);
                let _ = on_progress.send(result.clone());
                result
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

// ---------------------------------------------------------------- 行尾操作：揭示 / 复制

/// 在系统文件管理器中揭示文件所在位置（macOS 在 Finder 中选中，Windows 在资源管理器中选中）。
/// done 行传输出文件、其余行传原文件，由前端决定。
#[tauri::command]
pub fn reveal_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {path}"));
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .reveal_item_in_dir(&p)
        .map_err(|e| format!("无法打开文件管理器: {e}"))
}

/// 复制图片到系统剪贴板：读文件 → 解码为 RGBA 位图（与压缩管线同一套解码器）→ 写入剪贴板。
/// 支持 PNG / JPEG / WebP（WebP 即压缩产物格式）。
#[tauri::command]
pub fn copy_image(app: AppHandle, path: String) -> Result<(), String> {
    let data = std::fs::read(&path).map_err(|e| format!("读取失败: {e}"))?;
    let img = tinyimg::decode::decode_to_rgba(&data).map_err(|e| format!("无法解码图片: {e}"))?;
    let image = tauri::image::Image::new_owned(img.rgba, img.width, img.height);
    tauri_plugin_clipboard_manager::ClipboardExt::clipboard(&app)
        .write_image(&image)
        .map_err(|e| format!("写入剪贴板失败: {e}"))
}

// ---------------------------------------------------------------- 偏好

#[tauri::command]
pub fn get_prefs(app: AppHandle) -> Prefs {
    prefs::load(&app)
}

#[tauri::command]
pub fn save_prefs(app: AppHandle, prefs: Prefs) -> Result<(), String> {
    prefs::save(&app, &prefs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinyimg::compress::OutFormat;

    fn samples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/samples")
    }

    #[test]
    fn default_options_format_parsing() {
        assert_eq!(default_options("webp", 100).output_format, OutputFormat::WebP);
        assert_eq!(default_options("auto", 100).output_format, OutputFormat::Auto);
        // 无法识别 → Auto（与前端默认一致）
        assert_eq!(default_options("png", 100).output_format, OutputFormat::Auto);
        let o = default_options("webp", 100);
        assert_eq!((o.colors, o.quality, o.dither), (256, 88, false));
    }

    #[test]
    fn default_options_level_maps_to_params() {
        // 滑条两端：0 = CLI low 档参数，100 = CLI high 档参数（GUI 默认）
        let low = default_options("auto", 0);
        assert_eq!((low.colors, low.quality), (64, 72));
        let high = default_options("auto", 100);
        assert_eq!((high.colors, high.quality), (256, 88));
        // 越界 clamp 到 100
        let over = default_options("auto", 200);
        assert_eq!((over.colors, over.quality), (256, 88));
        // 中间值单调介于两端之间
        let mid = default_options("auto", 50);
        assert_eq!((mid.colors, mid.quality), (160, 80));
    }

    #[test]
    fn default_options_extreme_only_at_level_zero() {
        // 极致档（Zopfli）仅由滑条 = 0 触发；其余档位走 flate2 快速 zlib
        assert!(default_options("auto", 0).extreme, "level 0 应启用极致档");
        assert!(default_options("webp", 0).extreme, "webp + level 0 同样极致");
        assert!(!default_options("auto", 1).extreme, "level 1 不应极致");
        assert!(!default_options("auto", 100).extreme, "level 100 不应极致（GUI 默认）");
        assert!(!default_options("auto", 200).extreme, "越界 clamp 后也不应极致");
    }

    #[test]
    fn resolve_output_same_dir_never_overwrites_original() {
        let input = Path::new("/tmp/x/photo.png");
        // 输出目录 = 输入所在目录 → 退回 -min 后缀且不覆盖
        let (dir, overwrite) = resolve_output(input, &Some("/tmp/x".to_string()));
        assert!(dir.is_none(), "同目录不得直写输出目录");
        assert!(!overwrite, "同目录绝不覆盖原文件");
    }

    #[test]
    fn resolve_output_dedicated_dir_overwrites() {
        let input = Path::new("/tmp/x/photo.png");
        let (dir, overwrite) = resolve_output(input, &Some("/tmp/out".to_string()));
        assert_eq!(dir, Some(PathBuf::from("/tmp/out")));
        assert!(overwrite, "专用输出目录允许覆盖同名旧产物");
    }

    #[test]
    fn resolve_output_no_dir_falls_back_to_min_suffix() {
        let input = Path::new("/tmp/x/photo.png");
        let (dir, overwrite) = resolve_output(input, &None);
        assert!(dir.is_none());
        assert!(!overwrite);
    }

    #[test]
    fn collect_paths_expands_folder_and_filters() {
        let entries = collect_paths(vec![samples_dir().to_string_lossy().to_string()]);
        // samples 目录有 5 个样本（photo/smooth/logo/tiny.png + photo.jpg）
        assert_eq!(entries.len(), 5, "应递归收集全部样本: {:?}", entries.iter().map(|e| &e.name).collect::<Vec<_>>());
        assert!(entries.iter().all(|e| e.size > 0));
        assert!(entries.iter().any(|e| e.name == "logo.png"));
    }

    #[test]
    fn compress_one_produces_webp_in_dedicated_dir() {
        let input = samples_dir().join("logo.png");
        let out_dir = std::env::temp_dir().join("tinyimg-gui-test-out");
        let _ = std::fs::remove_dir_all(&out_dir);
        let opts = default_options("webp", 100);

        let r = compress_one(&input, &Some(out_dir.to_string_lossy().to_string()), &opts);

        assert_eq!(r.status, "done", "压缩应成功: {}", r.detail);
        assert_eq!(r.out_format, OutFormat::WebP.to_string());
        assert!(r.ratio > 0.5, "logo 扁平图走无损 WebP 应大幅缩小: {}", r.ratio);
        let out = PathBuf::from(r.output.unwrap());
        assert!(out.exists(), "输出文件应存在");
        assert_eq!(out.file_name().unwrap(), "logo.webp");
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn compress_one_same_dir_keeps_min_suffix() {
        // 输出目录 = 输入所在目录 → 走 -min 后缀，原文件不受影响
        let staging = std::env::temp_dir().join("tinyimg-gui-test-samedir");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        let input = staging.join("photo.png");
        std::fs::write(&input, std::fs::read(samples_dir().join("photo.png")).unwrap()).unwrap();
        let original_bytes = std::fs::read(&input).unwrap();

        let opts = default_options("webp", 100);
        let r = compress_one(&input, &Some(staging.to_string_lossy().to_string()), &opts);

        assert_eq!(r.status, "done");
        let out = PathBuf::from(r.output.clone().unwrap());
        assert_eq!(out.file_name().unwrap(), "photo-min.webp");
        assert_eq!(
            std::fs::read(&input).unwrap(),
            original_bytes,
            "原文件绝不能被覆盖"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn compress_one_rejects_non_image() {
        let staging = std::env::temp_dir().join("tinyimg-gui-test-bad");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        let input = staging.join("notes.txt");
        std::fs::write(&input, b"hello").unwrap();

        let r = compress_one(&input, &None, &default_options("webp", 100));
        assert_eq!(r.status, "failed");
        assert!(r.detail.contains("无法识别图片格式"), "错误信息应可读: {}", r.detail);
        let _ = std::fs::remove_dir_all(&staging);
    }
}
