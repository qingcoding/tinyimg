//! 图小小 TinyImg — CLI 入口
//!
//! 流程：解析参数 → 收集文件 → rayon 并行压缩 → 逐行报告 + 汇总统计

use std::path::Path;
use std::sync::Mutex;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use tinyimg::cli::Args;
use tinyimg::compress::{collect_files, process_file, Outcome};
use tinyimg::util::{bold, dim, format_ratio, format_size, green, pad_name, red, teal};

fn main() {
    let args = Args::parse();

    let opts = match args.to_options() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{} {}", red("参数错误:"), e);
            std::process::exit(2);
        }
    };

    let files: Vec<std::path::PathBuf> = collect_files(&args.paths, !args.no_recursive);
    if files.is_empty() {
        eprintln!("{} 未找到可处理的图片文件（支持 PNG / JPEG）", red("提示:"));
        std::process::exit(1);
    }

    println!("{}", bold("图小小 TinyImg v0.1.0 — 本地图片压缩"));
    println!(
        "{}",
        dim(&format!(
            "档位: {} 色 / JPEG q{} · 共 {} 个文件",
            opts.colors,
            opts.jpeg_quality,
            files.len()
        ))
    );

    // 进度条（文件数 > 1 时才有意义），画到 stderr，避免污染结果输出
    let progress = if files.len() > 1 {
        let pb = ProgressBar::new(files.len() as u64);
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len}")
                .unwrap()
                .progress_chars("█▓░"),
        );
        Some(pb)
    } else {
        None
    };

    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::with_capacity(files.len()));

    // 并行处理；rayon 默认用全部逻辑核心，可用 --jobs 限制
    let threads = args.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    });
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("创建线程池失败");

    pool.install(|| {
        files.par_iter().for_each(|input| {
            let outcome = process_file(input, args.output.as_deref(), args.overwrite, &opts);
            if let Some(pb) = &progress {
                pb.inc(1);
                pb.set_message(
                    input
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );
            }
            results.lock().unwrap().push(outcome);
        });
    });

    if let Some(pb) = &progress {
        pb.finish_and_clear();
    }

    // 按输入路径排序输出，结果稳定可读
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| outcome_path(a).cmp(outcome_path(b)));

    let mut done = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut total_original: u64 = 0;
    let mut total_compressed: u64 = 0;

    println!();
    for outcome in &results {
        match outcome {
            Outcome::Done {
                input,
                original,
                compressed,
                detail,
                ..
            } => {
                done += 1;
                total_original += original;
                total_compressed += compressed;
                let ratio = format_ratio(*original, *compressed);
                let ratio_colored = if compressed < original {
                    green(&ratio)
                } else {
                    red(&ratio)
                };
                println!(
                    "  {} {}  {} → {}  {}  {}",
                    teal("✓"),
                    pad_name(&file_name(input), 28),
                    format_size(*original),
                    format_size(*compressed),
                    ratio_colored,
                    dim(&format!("[{}]", detail)),
                );
            }
            Outcome::Skipped { input, reason } => {
                skipped += 1;
                println!(
                    "  {} {}  {}",
                    dim("·"),
                    pad_name(&file_name(input), 28),
                    dim(reason)
                );
            }
            Outcome::Failed { input, error } => {
                failed += 1;
                println!(
                    "  {} {}  {}",
                    red("✗"),
                    pad_name(&file_name(input), 28),
                    red(error)
                );
            }
        }
    }

    println!();
    println!("{}", dim("────────────────────────────────"));
    println!(
        "  共 {} 个 · {} 成功 · {} 跳过 · {} 失败",
        files.len(),
        green(&done.to_string()),
        dim(&skipped.to_string()),
        if failed > 0 {
            red(&failed.to_string())
        } else {
            dim("0")
        }
    );

    if done > 0 {
        let saved = total_original.saturating_sub(total_compressed);
        let avg = format_ratio(total_original, total_compressed);
        println!(
            "  {}（平均 {}）",
            bold(&format!("共节省 {}", format_size(saved))),
            teal(&avg)
        );
    }

    if failed > 0 {
        std::process::exit(1);
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn outcome_path(outcome: &Outcome) -> &Path {
    match outcome {
        Outcome::Done { input, .. } => input,
        Outcome::Skipped { input, .. } => input,
        Outcome::Failed { input, .. } => input,
    }
}
