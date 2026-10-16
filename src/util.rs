//! 通用工具：体积格式化、压缩率、终端颜色（尊重 NO_COLOR / 非 TTY）

use std::io::IsTerminal;
use std::sync::OnceLock;

/// 人类可读体积：B / KB / MB / GB（1024 进制，保留 1 位小数）
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", value, UNITS[unit])
}

/// 压缩率文本：负值表示体积减小（如 -70.0% 即节省 70%）
pub fn format_ratio(original: u64, compressed: u64) -> String {
    if original == 0 {
        return "0%".to_string();
    }
    let delta = (compressed as f64 - original as f64) / original as f64 * 100.0;
    format!("{:+.1}%", delta)
}

/// 是否启用 ANSI 颜色
fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

fn paint(text: &str, code: &str) -> String {
    if colors_enabled() {
        format!("\x1b[{}m{}\x1b[0m", code, text)
    } else {
        text.to_string()
    }
}

pub fn teal(text: &str) -> String {
    paint(text, "36")
}

pub fn green(text: &str) -> String {
    paint(text, "32")
}

pub fn red(text: &str) -> String {
    paint(text, "31")
}

pub fn dim(text: &str) -> String {
    paint(text, "2")
}

pub fn bold(text: &str) -> String {
    paint(text, "1")
}

/// 文件名右对齐补齐（按字符数计，中文按 2 宽近似）
pub fn pad_name(name: &str, width: usize) -> String {
    let mut display_width: usize = name.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    if display_width >= width {
        return name.to_string();
    }
    let mut result = name.to_string();
    while display_width < width {
        result.push(' ');
        display_width += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_works() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024 * 3 + 1024 * 512), "3.5 MB");
    }

    #[test]
    fn format_ratio_works() {
        assert_eq!(format_ratio(1000, 300), "-70.0%"); // 体积减小 70%
        assert_eq!(format_ratio(1000, 1200), "+20.0%"); // 体积反增
        assert_eq!(format_ratio(1000, 1000), "+0.0%");
        assert_eq!(format_ratio(0, 0), "0%");
    }
}
