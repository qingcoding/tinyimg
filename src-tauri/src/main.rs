#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            tinyimg_gui::commands::pick_files,
            tinyimg_gui::commands::pick_folder,
            tinyimg_gui::commands::pick_output_dir,
            tinyimg_gui::commands::collect_paths,
            tinyimg_gui::commands::compress_batch,
            tinyimg_gui::commands::get_prefs,
            tinyimg_gui::commands::save_prefs,
        ])
        .run(tauri::generate_context!())
        .expect("运行图小小 TinyImg 失败");
}
