//! 偏好持久化：记住上次打开位置（导入目录 / 输出目录）与输出格式选择。
//!
//! 存储于 `app_config_dir()/prefs.json`（macOS: `~/Library/Application
//! Support/cn.tinyimg.desktop/`），原子写入（临时文件 + rename）防半写损坏。

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
    /// 输出格式："webp"（默认）或 "auto"（跟随输入）
    pub output_format: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            last_input_dir: None,
            last_output_dir: None,
            output_format: "webp".to_string(),
        }
    }
}

impl Prefs {
    /// 解析输出格式；无法识别时回落 WebP（默认）
    pub fn output_format(&self) -> OutputFormat {
        match self.output_format.as_str() {
            "auto" | "a" | "自动" => OutputFormat::Auto,
            _ => OutputFormat::WebP,
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
    fn default_prefs_is_webp_and_empty() {
        let p = Prefs::default();
        assert_eq!(p.output_format, "webp");
        assert!(p.last_input_dir.is_none());
        assert!(p.last_output_dir.is_none());
        assert_eq!(p.output_format(), OutputFormat::WebP);
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
        // 无法识别 → 回落 WebP（默认）
        assert_eq!(
            Prefs { output_format: "png".into(), ..Default::default() }.output_format(),
            OutputFormat::WebP
        );
    }

    #[test]
    fn serialize_roundtrip_preserves_last_locations() {
        let prefs = Prefs {
            last_input_dir: Some("/Users/bruce/Pictures".into()),
            last_output_dir: Some("/Users/bruce/Desktop/out".into()),
            output_format: "auto".into(),
        };
        let text = serialize(&prefs).unwrap();
        assert!(text.contains("lastInputDir"), "序列化应为 camelCase: {text}");
        let back = deserialize(&text).unwrap();
        assert_eq!(back.last_input_dir.as_deref(), Some("/Users/bruce/Pictures"));
        assert_eq!(back.last_output_dir.as_deref(), Some("/Users/bruce/Desktop/out"));
        assert_eq!(back.output_format(), OutputFormat::Auto);
    }

    #[test]
    fn deserialize_corrupt_json_errors() {
        assert!(deserialize("{ not json").is_err());
    }

    #[test]
    fn prefs_file_path_under_base_dir() {
        assert_eq!(
            prefs_file(Path::new("/tmp/appconfig")),
            PathBuf::from("/tmp/appconfig/prefs.json")
        );
    }
}
