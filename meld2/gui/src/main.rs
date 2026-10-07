//! Meld desktop app: the same API and page as `meld2 serve`, run in this
//! process on a loopback port with a fresh token, shown in a window. One code
//! path: the window is a client of `meld2::serve` like any browser.
//!
//! Tray: Open and Quit. Closing the window while a run or a Minecraft server
//! is going hides it to the tray; otherwise it quits. A second launch shows
//! the first one's window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use meld2::serve;
use meld_core::state;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    let workspace = state::data_dir().join("workspace");
    std::fs::create_dir_all(&workspace).expect("creating the workspace folder");
    let token = serve::new_token().expect("reading the OS random source");
    let server = tiny_http::Server::http("127.0.0.1:0").expect("listening on loopback");
    let port = server.server_addr().to_ip().expect("an IP address").port();
    let ctx = serve::Ctx::new(workspace, token.clone(), None);
    let api = Arc::clone(&ctx);
    std::thread::spawn(move || serve::serve(server, api));
    let url = format!("http://127.0.0.1:{port}/?token={token}");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("Meld")
                .inner_size(1280.0, 820.0)
                .min_inner_size(1000.0, 650.0)
                .theme(Some(tauri::Theme::Dark))
                .build()?;
            let open = MenuItem::with_id(app, "open", "Open Meld", true, None::<&str>)?;
            let quit = MenuItem::with_id(
                app,
                "quit",
                "Quit (stops runs and servers)",
                true,
                None::<&str>,
            )?;
            TrayIconBuilder::with_id("meld")
                .icon(app.default_window_icon().cloned().expect("a bundle icon"))
                .tooltip("Meld")
                .menu(&Menu::with_items(app, &[&open, &quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        show(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(move |w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                if ctx.busy() {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("running Meld");
}
