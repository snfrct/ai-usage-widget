mod models;
mod providers;
mod scheduler;
mod store;

use models::AllUsage;
use store::Store;
use tauri::{AppHandle, Manager};

const WIDGET_LABEL: &str = "widget";

#[tauri::command]
fn get_usage(store: tauri::State<Store>) -> AllUsage {
    store.snapshot()
}

#[tauri::command]
async fn refresh_now(app: AppHandle) -> AllUsage {
    scheduler::refresh_all(&app).await
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(Store::new(&handle));

            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            if let Some(window) = app.get_webview_window(WIDGET_LABEL) {
                let _ = window.show();
            }

            scheduler::start(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_usage,
            refresh_now,
            quit_app,
            providers::claude::claude_login,
            providers::codex::codex_login,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
