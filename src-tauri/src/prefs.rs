//! 偏好持久化：记住上次打开位置（导入目录 / 输出目录）、输出格式、
//! 界面语言与主题配色。
//!
//! 存储于 `app_config_dir()/prefs.json`（macOS: `~/Library/Application
//! Support/cn.tinyimg.desktop/`），原子写入（临时文件 + rename）防半写损坏。
//! `language` / `theme` 由前端负责归一化与校验（合法值集合见 ui/i18n.js），
//! Rust 侧仅透传存储。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use tinyimg::compress::OutputFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// 上次导入（选择文件 / 文件夹）时所在目录，作为对话框默认目录
    pub last_input_dir: Option<String>,
    /// 上次选择的输出目录
    pub last_output_dir: Option<String>,
    /// 输出格式："auto"（默认，跟随输入）或 "webp"（一律转 WebP）
    pub output_format: String,
    /// 界面语言（BCP-47 归一化后的 code，如 "zh-CN" / "en"）；None = 跟随系统语言
    pub language: Option<String>,
    /// 主题配色 id（teal / dark / violet / ocean / rose / forest）；None 时前端回落默认
    pub theme: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            last_input_dir: None,
            last_output_dir: None,
            output_format: "auto".to_string(),
            language: None,
            theme: Some("teal".to_string()),
        }
    }
}

impl Prefs {
    /// 解析输出格式；显式 "webp" 保持 WebP（老用户 prefs.json 兼容），
    /// 无法识别时回落 Auto（默认）
    pub fn output_format(&self) -> OutputFormat {
        match self.output_format.as_str() {
            "auto" | "a" | "自动" => OutputFormat::Auto,
            "webp" => OutputFormat::WebP,
            _ => OutputFormat::Auto,
        }
    }
}

fn prefs_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| prefs_file(&d))
        .map_err(|e| format!("无法定位应用配置目录: {e}"))
}

/// 偏好文件路径（`<base>/prefs.json`），抽出来便于单测
pub fn prefs_file(base_dir: &Path) -> PathBuf {
    base_dir.join("prefs.json")
}

/// 序列化（单测 / 保存共用）
pub fn serialize(prefs: &Prefs) -> Result<String, String> {
    serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())
}

/// 反序列化（单测 / 读取共用）
pub fn deserialize(text: &str) -> Result<Prefs, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// 读取偏好；文件不存在或损坏时回落默认值（首次启动全空）
pub fn load(app: &AppHandle) -> Prefs {
    prefs_path(app)
        .and_then(|p| fs::read_to_string(p).map_err(|e| e.to_string()))
        .and_then(|text| deserialize(&text))
        .unwrap_or_default()
}

/// 保存偏好（原子写）
pub fn save(app: &AppHandle, prefs: &Prefs) -> Result<(), String> {
    let path = prefs_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = serialize(prefs)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &text).map_err(|e| format!("写入偏好失败: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("保存偏好失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefs_is_auto_and_empty() {
        let p = Prefs::default();
        assert_eq!(p.output_format, "auto");
        assert!(p.last_input_dir.is_none());
        assert!(p.last_output_dir.is_none());
        // 语言 / 主题默认值：跟随系统 + teal
        assert!(p.language.is_none());
        assert_eq!(p.theme.as_deref(), Some("teal"));
        assert_eq!(p.output_format(), OutputFormat::Auto);
    }

    #[test]
    fn output_format_parsing() {
        assert_eq!(
            Prefs { output_format: "auto".into(), ..Default::default() }.output_format(),
            OutputFormat::Auto
        );
        assert_eq!(
            Prefs { output_format: "自动".into(), ..Default::default() }.output_format(),
            OutputFormat::Auto
        );
        // 显式 "webp" 保持 WebP（向后兼容老用户 prefs.json）
        assert_eq!(
            Prefs { output_format: "webp".into(), ..Default::default() }.output_format(),
            OutputFormat::WebP
        );
        // 无法识别 → 回落 Auto（默认）
        assert_eq!(
            Prefs { output_format: "png".into(), ..Default::default() }.output_format(),
            OutputFormat::Auto
        );
    }

    #[test]
    fn serialize_roundtrip_preserves_last_locations() {
        let prefs = Prefs {
            last_input_dir: Some("/Users/bruce/Pictures".into()),
            last_output_dir: Some("/Users/bruce/Desktop/out".into()),
            output_format: "auto".into(),
            language: Some("en".into()),
            theme: Some("ocean".into()),
        };
        let text = serialize(&prefs).unwrap();
        assert!(text.contains("lastInputDir"), "序列化应为 camelCase: {text}");
        assert!(text.contains("\"language\": \"en\""), "language 应为 camelCase: {text}");
        assert!(text.contains("\"theme\": \"ocean\""), "theme 应为 camelCase: {text}");
        let back = deserialize(&text).unwrap();
        assert_eq!(back.last_input_dir.as_deref(), Some("/Users/bruce/Pictures"));
        assert_eq!(back.last_output_dir.as_deref(), Some("/Users/bruce/Desktop/out"));
        assert_eq!(back.output_format(), OutputFormat::Auto);
        assert_eq!(back.language.as_deref(), Some("en"));
        assert_eq!(back.theme.as_deref(), Some("ocean"));
    }

    #[test]
    fn deserialize_corrupt_json_errors() {
        assert!(deserialize("{ not json").is_err());
    }

    #[test]
    fn deserialize_legacy_json_fills_language_theme_defaults() {
        // 向后兼容：旧版偏好（无 language / theme 字段）反序列化后取默认值
        let back = deserialize(r#"{"lastInputDir":"/a","lastOutputDir":"/b","outputFormat":"webp"}"#)
            .unwrap();
        assert_eq!(back.last_input_dir.as_deref(), Some("/a"));
        assert_eq!(back.output_format(), OutputFormat::WebP);
        assert!(back.language.is_none());
        assert_eq!(back.theme.as_deref(), Some("teal"));
    }

    #[test]
    fn invalid_theme_value_passes_through() {
        // Rust 侧不做主题校验：非法值原样保存，由前端负责回落
        let prefs = Prefs {
            theme: Some("neon".into()),
            ..Default::default()
        };
        let back = deserialize(&serialize(&prefs).unwrap()).unwrap();
        assert_eq!(back.theme.as_deref(), Some("neon"));
    }

    #[test]
    fn prefs_file_path_under_base_dir() {
        assert_eq!(
            prefs_file(Path::new("/tmp/appconfig")),
            PathBuf::from("/tmp/appconfig/prefs.json")
        );
    }
}
