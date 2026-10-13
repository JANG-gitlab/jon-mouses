#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod diagnostics;
mod discovery;
mod engine;
mod layout;
mod permissions;
mod screen;
mod store;
mod texts;
mod ui;
mod wifi;
mod wire;

use std::thread;

use tauri::RunEvent;
use tokio::sync::mpsc;

fn main() {
    let (commands, inbox) = mpsc::unbounded_channel();
    tauri::Builder::default()
        .manage(commands)
        .invoke_handler(tauri::generate_handler![ui::submit_code, ui::cancel_pairing, ui::layout_state, ui::arrange, ui::resize, ui::identify_screens])
        .on_window_event(ui::on_window_event)
        .setup(|app| {
            ui::install_tray(app.handle())?;
            let handle = app.handle().clone();
            thread::spawn(move || engine::run(handle, inbox));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start Jon Mouse's")
        .run(|_, event| {
            if let RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
