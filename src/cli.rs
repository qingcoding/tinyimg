//! 命令行参数定义

use std::path::PathBuf;

use clap::Parser;

use crate::compress::{Options, OutputFormat};

/// TinyImg — 本地图片压缩工具（TinyPNG 核心算法的 Rust 实现）
#[derive(Parser, Debug)]
#[command(
    name = "tinyimg",
    version,
    about = "TinyImg — 本地批量图片压缩工具（PNG 量化 + JPEG 重编码 + WebP 输出）",
    after_help = "示例:\n  tinyimg photo.png                  # 压缩单张，输出 photo-min.png\n  tinyimg ./assets -o ./out          # 压缩目录（递归），输出到 out/\n  tinyimg *.png --preset low         # 低档位，体积优先\n  tinyimg a.png --colors 64 --dither # 自定义 64 色 + 误差扩散\n  tinyimg ./assets --format webp     # 一律输出 WebP（有损）"
)]
pub struct Args {
    /// 图片文件或目录（目录默认递归扫描，仅识别 PNG/JPEG）
    #[arg(required = true, value_name = "PATHS")]
    pub paths: Vec<PathBuf>,

    /// 输出目录（默认：原文件同目录，文件名加 -min 后缀，绝不覆盖原文件）
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// 质量档位：high(256色/q88) / medium(128色/q82) / low(64色/q72)
    #[arg(short, long, default_value = "high", value_name = "PRESET")]
    pub preset: String,

    /// 输出格式：auto（跟随输入，默认）/ webp（一律转 WebP 有损）
    #[arg(short, long, default_value = "auto", value_name = "FMT")]
    pub format: String,

    /// 自定义 PNG 量化色数（2-256），设置后覆盖档位色数
    #[arg(long, value_name = "N")]
    pub colors: Option<usize>,

    /// 自定义有损质量（1-100），作用于 JPEG 重编码 / WebP 编码
    #[arg(long, value_name = "Q")]
    pub jpeg_quality: Option<u8>,

    /// 启用 Floyd–Steinberg 误差扩散（照片更平滑，体积略增）
    #[arg(long)]
    pub dither: bool,

    /// 覆盖模式：原地替换原文件（危险操作，请谨慎）
    #[arg(long)]
    pub overwrite: bool,

    /// 目录不递归，仅处理顶层文件
    #[arg(long)]
    pub no_recursive: bool,

    /// 并行任务数（默认 = CPU 逻辑核心数）
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,
}

impl Args {
    /// 将 CLI 参数解析为压缩选项
    pub fn to_options(&self) -> anyhow::Result<Options> {
        let (mut colors, mut quality) = match self.preset.to_lowercase().as_str() {
            "high" | "h" | "高" => (256, 88),
            "medium" | "m" | "中" => (128, 82),
            "low" | "l" | "低" => (64, 72),
            other => {
                anyhow::bail!("未知质量档位: {}（可选: high / medium / low）", other)
            }
        };

        let output_format = match self.format.to_lowercase().as_str() {
            "auto" | "a" | "自动" => OutputFormat::Auto,
            "webp" | "w" => OutputFormat::WebP,
            other => {
                anyhow::bail!("未知输出格式: {}（可选: auto / webp）", other)
            }
        };

        if let Some(c) = self.colors {
            if !(2..=256).contains(&c) {
                anyhow::bail!("--colors 必须在 2-256 之间");
            }
            colors = c;
        }
        if let Some(q) = self.jpeg_quality {
            if !(1..=100).contains(&q) {
                anyhow::bail!("--jpeg-quality 必须在 1-100 之间");
            }
            quality = q;
        }

        Ok(Options {
            colors,
            quality,
            dither: self.dither,
            output_format,
        })
    }
}
