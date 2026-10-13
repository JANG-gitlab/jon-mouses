use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent, Wry,
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::{
    engine::Command,
    permissions::{self, Permission},
    store::Side,
    texts,
    wire::Key,
};

const TRAY_ID: &str = "jon-mouses";

type Task = Box<dyn FnOnce(&AppHandle) + Send>;

/// Tauri waits for the main thread on every tray or window call. Running them
/// here keeps the input path from ever stalling behind an open menu, which
/// would let the macOS event tap back up and freeze the local mouse.
#[derive(Clone)]
pub struct Worker(std::sync::mpsc::Sender<Task>);

impl Worker {
    pub fn spawn(app: AppHandle) -> Self {
        let (tasks, queue) = std::sync::mpsc::channel::<Task>();
        std::thread::spawn(move || {
            for task in queue {
                task(&app);
            }
        });
        Self(tasks)
    }

    pub fn run(&self, task: impl FnOnce(&AppHandle) + Send + 'static) {
        let _ = self.0.send(Box::new(task));
    }
}
const PAIRING_WINDOW: &str = "pairing";
const LAYOUT_WINDOW: &str = "layout";
const IDENTIFY_WINDOW: &str = "identify";
const PAIR_PREFIX: &str = "pair:";

#[derive(Clone, Copy, PartialEq)]
pub enum Status {
    NeedsPermission,
    Ready,
    Active,
}

#[derive(Clone, PartialEq)]
pub struct PeerView {
    pub name: String,
    pub latency_ms: Option<Option<u64>>,
}

#[derive(Clone, PartialEq)]
pub struct View {
    pub status: Status,
    pub peers: Vec<PeerView>,
    pub nearby: Vec<(Key, String)>,
    pub missing: Vec<Permission>,
    pub offer_wifi_fix: bool,
}

impl View {
    pub fn waiting_for(missing: Vec<Permission>) -> Self {
        Self { status: Status::NeedsPermission, peers: Vec::new(), nearby: Vec::new(), missing, offer_wifi_fix: false }
    }

    fn starting() -> Self {
        Self { status: Status::Ready, peers: Vec::new(), nearby: Vec::new(), missing: Vec::new(), offer_wifi_fix: false }
    }
}

fn icon(status: Status) -> Image<'static> {
    let bytes: &[u8] = match status {
        Status::Active => include_bytes!("../tray/actif.png"),
        _ => include_bytes!("../tray/veille.png"),
    };
    Image::from_bytes(bytes).expect("embedded tray icon")
}

fn menu(app: &AppHandle, view: &View) -> tauri::Result<Menu<Wry>> {
    let texts = texts::get();
    let menu = Menu::new(app)?;
    let line = |id: &str, label: &str, enabled: bool| MenuItem::with_id(app, id, label, enabled, None::<&str>);

    menu.append(&line("status", texts.status(view.status), false)?)?;
    for permission in &view.missing {
        let (id, label) = match permission {
            Permission::Accessibility => ("allow:accessibility", texts.allow_accessibility),
            Permission::InputMonitoring => ("allow:input", texts.allow_input_monitoring),
        };
        menu.append(&line(id, label, true)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    for peer in &view.peers {
        let state = match peer.latency_ms {
            None => texts.offline.to_string(),
            Some(None) => texts.connecting.to_string(),
            Some(Some(0)) => "< 1 ms".to_string(),
            Some(Some(ms)) => format!("{ms} ms"),
        };
        menu.append(&line("peer", &format!("{} · {state}", peer.name), false)?)?;
    }
    for (key, name) in &view.nearby {
        let id = format!("{PAIR_PREFIX}{}", hex::encode(key));
        menu.append(&line(&id, &texts.pair(name), true)?)?;
    }
    if view.peers.is_empty() && view.nearby.is_empty() && view.status != Status::NeedsPermission {
        menu.append(&line("searching", texts.searching, false)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if !view.peers.is_empty() {
        menu.append(&line("layout", texts.layout, true)?)?;
    }
    if view.offer_wifi_fix {
        menu.append(&line("wifi-fix", texts.wifi_fix, true)?)?;
    }
    menu.append(&line("quit", texts.quit, true)?)?;
    Ok(menu)
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    let commands: State<UnboundedSender<Command>> = app.state();
    if let Some(key) = id.strip_prefix(PAIR_PREFIX).and_then(|hex| hex::decode(hex).ok()) {
        if let Ok(key) = key.try_into() {
            let _ = commands.send(Command::Pair(key));
        }
        return;
    }
    match id {
        "allow:accessibility" => permissions::request(Permission::Accessibility),
        "allow:input" => permissions::request(Permission::InputMonitoring),
        "layout" => open_layout(app),
        "wifi-fix" => {
            let _ = commands.send(Command::InstallWifiFix);
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn open_layout(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LAYOUT_WINDOW) {
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LAYOUT_WINDOW, WebviewUrl::App("layout.html".into()))
        .title(texts::get().layout.trim_end_matches('…'))
        .inner_size(760.0, 540.0)
        .min_inner_size(480.0, 400.0)
        .center()
        .focused(true)
        .build();
    if let Err(error) = built {
        eprintln!("cannot open the layout window: {error}");
    }
}

pub fn layout_changed(app: &AppHandle) {
    let _ = app.emit_to(LAYOUT_WINDOW, "layout-changed", ());
}

pub fn identify(app: &AppHandle, number: u8) {
    close_identify(app);
    let built = WebviewWindowBuilder::new(app, IDENTIFY_WINDOW, WebviewUrl::App("identify.html".into()))
        .title("Jon Mouse's")
        .inner_size(320.0, 320.0)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .center()
        .initialization_script(format!("window.screenNumber = {number};"))
        .build();
    if let Err(error) = built {
        eprintln!("cannot show the screen number: {error}");
    }
}

pub fn close_identify(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(IDENTIFY_WINDOW) {
        let _ = window.destroy();
    }
}

pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let view = View::starting();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(view.status))
        .icon_as_template(true)
        .tooltip(texts::get().status(view.status))
        .menu(&menu(app, &view)?)
        .on_menu_event(on_menu)
        .build(app)?;
    Ok(())
}

pub fn refresh(app: &AppHandle, view: &View) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.set_icon(Some(icon(view.status)));
    let _ = tray.set_icon_as_template(true);
    let _ = tray.set_tooltip(Some(texts::get().status(view.status)));
    if let Ok(menu) = menu(app, view) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn open_pairing(app: &AppHandle, data: serde_json::Value) {
    close_pairing(app);
    let built = WebviewWindowBuilder::new(app, PAIRING_WINDOW, WebviewUrl::App("index.html".into()))
        .title("Jon Mouse's")
        .inner_size(380.0, 280.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .always_on_top(true)
        .focused(true)
        .initialization_script(format!("window.pairing = {data};"))
        .build();
    if let Err(error) = built {
        eprintln!("cannot open the pairing window: {error}");
    }
}

pub fn show_code(app: &AppHandle, code: &str, name: &str) {
    open_pairing(app, serde_json::json!({ "mode": "show", "code": code, "name": name }));
}

pub fn ask_code(app: &AppHandle, name: &str) {
    open_pairing(app, serde_json::json!({ "mode": "ask", "name": name }));
}

pub fn pairing_failed(app: &AppHandle) {
    let _ = app.emit_to(PAIRING_WINDOW, "pairing-failed", ());
}

pub fn close_pairing(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PAIRING_WINDOW) {
        let _ = window.destroy();
    }
}

pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() == PAIRING_WINDOW && matches!(event, WindowEvent::CloseRequested { .. }) {
        let commands: State<UnboundedSender<Command>> = window.state();
        let _ = commands.send(Command::CancelPairing);
    }
}

#[tauri::command]
pub fn submit_code(code: String, commands: State<UnboundedSender<Command>>) {
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    let _ = commands.send(Command::SubmitCode(digits));
}

#[tauri::command]
pub fn cancel_pairing(app: AppHandle, commands: State<UnboundedSender<Command>>) {
    let _ = commands.send(Command::CancelPairing);
    close_pairing(&app);
}

fn parse_key(key: &str) -> Result<Key, String> {
    hex::decode(key).ok().and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| "unknown device".to_string())
}

#[tauri::command]
pub async fn layout_state(commands: State<'_, UnboundedSender<Command>>) -> Result<serde_json::Value, String> {
    let (reply, state) = oneshot::channel();
    commands.send(Command::Layout(reply)).map_err(|error| error.to_string())?;
    state.await.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn arrange(key: String, side: Side, offset_mm: f32, commands: State<UnboundedSender<Command>>) -> Result<(), String> {
    let key = parse_key(&key)?;
    commands.send(Command::Arrange { key, side, offset_mm }).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn resize(key: Option<String>, size_mm: Option<(f32, f32)>, commands: State<UnboundedSender<Command>>) -> Result<(), String> {
    let key = key.as_deref().map(parse_key).transpose()?;
    commands.send(Command::Resize { key, size_mm }).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn identify_screens(commands: State<UnboundedSender<Command>>) {
    let _ = commands.send(Command::Identify);
}
