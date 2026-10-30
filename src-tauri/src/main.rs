// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod crypto;
mod game;
mod host;
mod settings;
mod updater;

use tauri::Manager;

fn main() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
                let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
            }
        }))
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.set_decorations(false).ok();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host::ready,
            host::login,
            host::register,
            host::logout,
            host::refresh_me,
            host::refresh_servers,
            host::friends_poll,
            host::report,
            host::match_result,
            host::list_sessions,
            host::play,
            host::minimize,
            host::maximize,
            host::close,
            host::close_anyway,
            host::drag,
            host::friend,
            host::join,
            host::host_public,
            host::max_progress,
            host::set_username,
            host::set_password,
            host::set_email,
            host::confirm_email,
            host::redeem,
            host::patchnotes,
            host::warning_seen,
            host::leave_game,
            host::delete_account,
            host::delete_cancel,
            host::quickplay_choice,
            host::set_language,
            host::servers_tab,
            host::page_error,
            host::pick_folder,
            host::open_data,
            host::uninstall,
            host::update_launcher,
            host::discord_link,
            host::bug,
            host::dlc,
            host::dlc_refresh,
            host::resize,
            host::refresh_server_status,
            host::check_update_status,
            api::get_me,
            api::get_profile,
            api::post_profile,
            api::presence,
            api::sync_profile,
            game::detect_install,
            game::set_identity,
            game::launch_game,
            updater::check_updates,
            settings::load_settings,
            settings::save_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
