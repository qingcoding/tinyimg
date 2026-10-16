//! 压缩管线：格式识别、PNG/JPEG 分流、单文件处理与输出策略

pub mod jpeg;
pub mod png;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// 支持的图片格式（按文件头魔术字节识别）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
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
    /// JPEG 重编码质量（1-100）
    pub jpeg_quality: u8,
    /// 是否启用 Floyd–Steinberg 误差扩散
    pub dither: bool,
}

/// 压缩产物：字节流 + 人类可读的处理说明
pub struct Compressed {
    pub bytes: Vec<u8>,
    pub detail: String,
}

/// 压缩内存中的图片字节
pub fn compress_bytes(data: &[u8], opts: &Options) -> Result<Compressed> {
    match detect_format(data) {
        Some(Format::Png) => png::compress_png(data, opts),
        Some(Format::Jpeg) => jpeg::compress_jpeg(data, opts),
        None => bail!("无法识别图片格式（仅支持 PNG / JPEG）"),
    }
}

/// 单文件处理结果
pub enum Outcome {
    Done {
        input: PathBuf,
        output: PathBuf,
        original: u64,
        compressed: u64,
        detail: String,
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

    let output = match output_path(input, output_dir, overwrite) {
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
    }
}

/// 输出路径策略：
/// - 指定 `--output DIR`：写入 DIR/原文件名
/// - `--overwrite`：原地替换
/// - 默认：同目录 `<原名>-min.<扩展名>`（绝不覆盖原文件）
fn output_path(input: &Path, output_dir: Option<&Path>, overwrite: bool) -> Result<PathBuf> {
    if let Some(dir) = output_dir {
        let name = input
            .file_name()
            .context("无法获取文件名")?;
        return Ok(dir.join(name));
    }
    if overwrite {
        return Ok(input.to_path_buf());
    }
    let stem = input.file_stem().context("无法获取文件主名")?;
    let ext = input.extension().context("无法获取扩展名")?;
    let name = format!(
        "{}-min.{}",
        stem.to_string_lossy(),
        ext.to_string_lossy()
    );
    Ok(input.with_file_name(name))
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
