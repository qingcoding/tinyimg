//! 压缩管线：格式识别、PNG/JPEG 分流、单文件处理与输出策略

pub mod jpeg;
pub mod png;
pub mod webp;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// 支持的输入图片格式（按文件头魔术字节识别）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

/// 输出格式选择（CLI --format）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// 跟随输入格式（PNG→PNG，JPEG→JPEG）
    #[default]
    Auto,
    /// 一律输出 WebP（有损，质量跟随档位）
    WebP,
}

/// 实际输出格式（用于报告）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutFormat {
    Png,
    Jpeg,
    WebP,
}

impl std::fmt::Display for OutFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutFormat::Png => write!(f, "PNG"),
            OutFormat::Jpeg => write!(f, "JPEG"),
            OutFormat::WebP => write!(f, "WebP"),
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Format::Png => write!(f, "PNG"),
            Format::Jpeg => write!(f, "JPEG"),
        }
    }
}

/// 图片信息（输入侧元数据 + 实际输出格式）
#[derive(Debug, Clone, Copy)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub in_format: Format,
    pub out_format: OutFormat,
}

/// 识别图片格式。仅凭文件头判断，不信任扩展名。
pub fn detect_format(data: &[u8]) -> Option<Format> {
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() >= 8 && data[..8] == PNG_SIG {
        return Some(Format::Png);
    }
    // JPEG: FF D8 FF
    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return Some(Format::Jpeg);
    }
    None
}

/// 压缩参数（由 CLI 档位或自定义值填充）
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// PNG 量化色数上限（2-256）
    pub colors: usize,
    /// 有损质量（1-100）：JPEG 重编码 / WebP 编码共用
    pub quality: u8,
    /// 是否启用 Floyd–Steinberg 误差扩散
    pub dither: bool,
    /// 输出格式
    pub output_format: OutputFormat,
}

/// 压缩产物：字节流 + 人类可读的处理说明 + 图片信息
pub struct Compressed {
    pub bytes: Vec<u8>,
    pub detail: String,
    pub info: ImageInfo,
}

/// 压缩内存中的图片字节
pub fn compress_bytes(data: &[u8], opts: &Options) -> Result<Compressed> {
    match detect_format(data) {
        Some(Format::Png) => png::compress_png(data, opts),
        Some(Format::Jpeg) => jpeg::compress_jpeg(data, opts),
        None => bail!("无法识别图片格式（仅支持 PNG / JPEG）"),
    }
}

/// 扁平少色图检测（抽样统计唯一颜色数）
///
/// Logo / 图标 / 像素画等硬边图 ≤ 256 色：有损 WebP 会糊边，
/// 此类图应走无损编码守住"视觉无损"承诺；照片类必然超阈值走有损。
pub fn is_flat_art(rgba: &[u8]) -> bool {
    const SAMPLE_CAP: usize = 50_000;
    const FLAT_THRESHOLD: usize = 256;

    let total = rgba.len() / 4;
    if total == 0 {
        return true;
    }
    let step = (total / SAMPLE_CAP).max(1);
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut i = 0;
    while i < total {
        let p = &rgba[i * 4..i * 4 + 4];
        let key = (p[0] as u32) << 24 | (p[1] as u32) << 16 | (p[2] as u32) << 8 | p[3] as u32;
        seen.insert(key);
        if seen.len() > FLAT_THRESHOLD {
            return false;
        }
        i += step;
    }
    true
}

/// 单文件处理结果
pub enum Outcome {
    Done {
        input: PathBuf,
        output: PathBuf,
        original: u64,
        compressed: u64,
        detail: String,
        info: ImageInfo,
    },
    Skipped {
        input: PathBuf,
        reason: String,
    },
    Failed {
        input: PathBuf,
        error: String,
    },
}

/// 处理单个文件：读取 → 压缩 → 体积守卫 → 落盘
pub fn process_file(
    input: &Path,
    output_dir: Option<&Path>,
    overwrite: bool,
    opts: &Options,
) -> Outcome {
    let data = match fs::read(input) {
        Ok(d) => d,
        Err(e) => {
            return Outcome::Failed {
                input: input.to_path_buf(),
                error: format!("读取失败: {e}"),
            }
        }
    };
    let original = data.len() as u64;

    let compressed = match compress_bytes(&data, opts) {
        Ok(c) => c,
        Err(e) => {
            return Outcome::Failed {
                input: input.to_path_buf(),
                error: format!("{e:#}"),
            }
        }
    };

    // 体积守卫：压缩没有收益时保留原文件（TinyPNG 同样会告知"已优化"）
    if compressed.bytes.len() as u64 >= original {
        return Outcome::Skipped {
            input: input.to_path_buf(),
            reason: "压缩后体积未减小，保留原文件".to_string(),
        };
    }

    let output = match output_path(input, output_dir, overwrite, compressed.info.out_format) {
        Ok(p) => p,
        Err(e) => {
            return Outcome::Failed {
                input: input.to_path_buf(),
                error: e.to_string(),
            }
        }
    };

    if output.exists() && !overwrite {
        return Outcome::Skipped {
            input: input.to_path_buf(),
            reason: "输出文件已存在（--overwrite 可覆盖）".to_string(),
        };
    }

    if let Some(parent) = output.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return Outcome::Failed {
                input: input.to_path_buf(),
                error: format!("创建输出目录失败: {e}"),
            };
        }
    }

    if let Err(e) = fs::write(&output, &compressed.bytes) {
        return Outcome::Failed {
            input: input.to_path_buf(),
            error: format!("写入失败: {e}"),
        };
    }

    Outcome::Done {
        input: input.to_path_buf(),
        output,
        original,
        compressed: compressed.bytes.len() as u64,
        detail: compressed.detail,
        info: compressed.info,
    }
}

/// 输出路径策略：
/// - 指定 `--output DIR`：写入 DIR/<输出格式对应的文件名>
/// - `--overwrite`：原地替换
/// - 默认：同目录 `<原名>-min.<输出扩展名>`（绝不覆盖原文件）
///
/// 输出格式为 WebP 时扩展名替换为 .webp（如 photo.png → photo-min.webp）
fn output_path(
    input: &Path,
    output_dir: Option<&Path>,
    overwrite: bool,
    out_format: OutFormat,
) -> Result<PathBuf> {
    if let Some(dir) = output_dir {
        let stem = input.file_stem().context("无法获取文件主名")?;
        let name = format!("{}.{}", stem.to_string_lossy(), out_ext(out_format));
        return Ok(dir.join(name));
    }
    if overwrite {
        // 原地替换但格式变化时（如 PNG→WebP）不能覆盖原文件，改用新扩展名
        if out_format != OutFormat::Png && out_format != OutFormat::Jpeg {
            let stem = input.file_stem().context("无法获取文件主名")?;
            let name = format!("{}.{}", stem.to_string_lossy(), out_ext(out_format));
            return Ok(input.with_file_name(name));
        }
        return Ok(input.to_path_buf());
    }
    let stem = input.file_stem().context("无法获取文件主名")?;
    let name = format!(
        "{}-min.{}",
        stem.to_string_lossy(),
        out_ext(out_format)
    );
    Ok(input.with_file_name(name))
}

fn out_ext(format: OutFormat) -> &'static str {
    match format {
        OutFormat::Png => "png",
        OutFormat::Jpeg => "jpg",
        OutFormat::WebP => "webp",
    }
}

/// 收集待处理文件：文件直接入列，目录按扩展名过滤扫描
pub fn collect_files(paths: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut walker = walkdir::WalkDir::new(path);
            if !recursive {
                walker = walker.max_depth(1);
            }
            for entry in walker.into_iter().filter_map(Result::ok) {
                let p = entry.path();
                if p.is_file() && is_supported_ext(p) {
                    files.push(p.to_path_buf());
                }
            }
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    files
}

fn is_supported_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref(),
        Some("png") | Some("jpg") | Some("jpeg")
    )
}
