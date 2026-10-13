use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    rc::Rc,
    time::Duration,
};

use futures::StreamExt;
use input_capture::{CaptureEvent, CaptureHandle, InputCapture};
use input_emulation::InputEmulation;
use input_event::{
    Event, KeyboardEvent, PointerEvent,
    scancode::Linux::{self, KeyEsc, KeyLeftAlt, KeyLeftCtrl},
};
use snow::StatelessTransportState;
use tauri::{AppHandle, Manager};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, oneshot},
    task::{JoinHandle, LocalSet, spawn_local},
    time::{Instant, interval, sleep, sleep_until, timeout},
};

use crate::{
    clipboard,
    diagnostics::{Cadence, Log},
    discovery::{self, Discovery},
    layout, permissions,
    screen::{self, Screen},
    store::{Peer, Side, Store},
    wifi,
    ui::{self, PeerView, Status, View},
    wire::{self, Error, Key, Message},
};

const RELEASE_CHORD: [Linux; 3] = [KeyLeftCtrl, KeyLeftAlt, KeyEsc];
const RELEASE_HOLD: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const FAILURE_DISPLAY: Duration = Duration::from_millis(2500);
const IDENTIFY_DISPLAY: Duration = Duration::from_millis(2500);
const DATAGRAM_SIZE: usize = 1500;
const SILENCE_LIMIT: Duration = Duration::from_secs(3);
const MOTION_INTERVAL: Duration = Duration::from_millis(4);
const EMULATION_TIMEOUT: Duration = Duration::from_millis(500);
const CLIPBOARD_LIMIT: u32 = 32 * 1024 * 1024;

pub enum Command {
    Pair(Key),
    SubmitCode(String),
    CancelPairing,
    Layout(oneshot::Sender<serde_json::Value>),
    Arrange { key: Key, side: Side, offset_mm: f32 },
    Resize { key: Option<Key>, size_mm: Option<(f32, f32)> },
    Identify,
    InstallWifiFix,
}

enum Signal {
    Incoming(TcpStream),
    PairRequest(TcpStream, String),
    Session(Key, TcpStream, StatelessTransportState),
    ConnectFailed(Key),
    Paired(Peer),
    PairingFailed,
    Received(Key, Message),
    Closed(Key, u64),
    ClipboardRead(Key, clipboard::Snapshot),
    ClipboardWritten(Result<Option<u64>, String>),
    WifiFix(wifi::Outcome),
}

struct Nearby {
    name: String,
    addr: SocketAddr,
    service: String,
}

struct Link {
    id: u64,
    outbox: mpsc::UnboundedSender<Message>,
    channel: Rc<wire::Channel>,
    ip: IpAddr,
    datagram_address: Option<SocketAddr>,
    datagrams_reach_peer: bool,
    datagrams_reach_us: bool,
    latency_us: Option<u64>,
    heard_at: Instant,
    clipboard_incoming: Vec<u8>,
}

struct Engine {
    ui: ui::Worker,
    name: String,
    store: Store,
    capture: InputCapture,
    emulation: InputEmulation,
    signals: mpsc::UnboundedSender<Signal>,
    nearby: HashMap<Key, Nearby>,
    links: HashMap<Key, Link>,
    connecting: HashSet<Key>,
    next_link: u64,
    controlling: Option<Key>,
    controlled_by: Option<Key>,
    pairing: Option<JoinHandle<()>>,
    code_entry: Option<oneshot::Sender<String>>,
    release_at: Option<Instant>,
    started: Instant,
    shown: Option<View>,
    screen: Screen,
    datagrams: Rc<UdpSocket>,
    pending_motion: Option<(Key, f64, f64)>,
    motion_due: Option<Instant>,
    motion_sent_at: Instant,
    sent_motion: Cadence,
    received_motion: Cadence,
    log: Log,
    clipboard_known: Option<u64>,
    data_dir: std::path::PathBuf,
    wifi_fix_installed: bool,
}

pub fn run(app: AppHandle, commands: mpsc::UnboundedReceiver<Command>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("engine runtime");
    LocalSet::new().block_on(&runtime, async move {
        if let Err(error) = start(app, commands).await {
            eprintln!("Jon Mouse's stopped: {error}");
        }
    });
}

async fn start(app: AppHandle, mut commands: mpsc::UnboundedReceiver<Command>) -> Result<(), Error> {
    let worker = ui::Worker::spawn(app.clone());
    let mut missing = permissions::missing();
    while !missing.is_empty() {
        let view = View::waiting_for(missing.clone());
        worker.run(move |app| ui::refresh(app, &view));
        while permissions::missing() == missing {
            sleep(TICK).await;
        }
        missing = permissions::missing();
    }

    let data_dir = app.path().app_data_dir()?;
    let store = Store::load(data_dir.clone())?;
    let capture = InputCapture::new(None).await?;
    let emulation = InputEmulation::new(None).await?;
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let datagrams = Rc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let name = gethostname::gethostname()
        .to_string_lossy()
        .trim_end_matches(".local")
        .to_string();
    let (_daemon, browse) = discovery::advertise(&store.public, &name, listener.local_addr()?.port())?;

    let (signals, mut inbox) = mpsc::unbounded_channel();
    spawn_local(accept(listener, signals.clone()));

    let mut engine = Engine {
        ui: worker,
        name,
        store,
        capture,
        emulation,
        signals,
        nearby: HashMap::new(),
        links: HashMap::new(),
        connecting: HashSet::new(),
        next_link: 0,
        controlling: None,
        controlled_by: None,
        pairing: None,
        code_entry: None,
        release_at: None,
        started: Instant::now(),
        shown: None,
        screen: screen::detect(),
        datagrams: datagrams.clone(),
        pending_motion: None,
        motion_due: None,
        motion_sent_at: Instant::now(),
        sent_motion: Cadence::default(),
        received_motion: Cadence::default(),
        log: Log::open(&data_dir),
        clipboard_known: None,
        data_dir,
        wifi_fix_installed: wifi::helper_installed(),
    };
    engine.log.write("started");
    if !engine.wifi_fix_installed && !engine.store.settings.wifi_fix_declined {
        engine.install_wifi_fix();
    }
    engine.apply_screen_override();
    for peer in engine.store.peers.clone() {
        engine.track(&peer).await;
    }
    engine.refresh();

    let mut ticker = interval(TICK);
    let mut packet = [0u8; DATAGRAM_SIZE];
    loop {
        tokio::select! {
            event = engine.capture.next() => match event {
                Some(Ok((handle, event))) => engine.on_capture(handle, event).await,
                Some(Err(error)) => return Err(error.into()),
                None => return Ok(()),
            },
            Some(signal) = inbox.recv() => engine.on_signal(signal).await,
            Some(command) = commands.recv() => engine.on_command(command).await,
            Ok(event) = browse.recv_async() => engine.on_discovery(event),
            Ok((len, from)) = datagrams.recv_from(&mut packet) => engine.on_datagram(from, &packet[..len]).await,
            _ = ticker.tick() => engine.on_tick().await,
            _ = sleep_until(engine.motion_due.unwrap_or_else(Instant::now)), if engine.motion_due.is_some() => {
                engine.flush_motion();
            }
            _ = sleep_until(engine.release_at.unwrap_or_else(Instant::now)), if engine.release_at.is_some() => {
                engine.release().await;
            }
        }
    }
}

async fn accept(listener: TcpListener, signals: mpsc::UnboundedSender<Signal>) {
    while let Ok((stream, _)) = listener.accept().await {
        if signals.send(Signal::Incoming(stream)).is_err() {
            break;
        }
    }
}

async fn triage(mut stream: TcpStream, private: Key, known: Vec<Key>, signals: mpsc::UnboundedSender<Signal>) {
    let _ = stream.set_nodelay(true);
    let signal = match stream.read_u8().await {
        Ok(wire::SESSION_REQUEST) => match wire::accept_session(stream, &private, &known).await {
            Ok((key, stream, transport)) => Signal::Session(key, stream, transport),
            Err(_) => return,
        },
        Ok(wire::PAIR_REQUEST) => match wire::read_pairing_name(&mut stream).await {
            Ok(name) => Signal::PairRequest(stream, name),
            Err(_) => return,
        },
        _ => return,
    };
    let _ = signals.send(signal);
}

async fn connect(addr: SocketAddr, private: Key, key: Key, signals: mpsc::UnboundedSender<Signal>) {
    let session = timeout(CONNECT_TIMEOUT, async {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        wire::open_session(stream, &private, &key).await
    })
    .await;
    let _ = signals.send(match session {
        Ok(Ok((stream, transport))) => Signal::Session(key, stream, transport),
        _ => Signal::ConnectFailed(key),
    });
}

impl Engine {
    fn now(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    fn send(&self, key: &Key, message: Message) {
        if let Some(link) = self.links.get(key) {
            let _ = link.outbox.send(message);
        }
    }

    fn send_datagram(&self, link: &Link, message: &Message) -> bool {
        let Some(address) = link.datagram_address else {
            return false;
        };
        match link.channel.seal_datagram(message) {
            Ok(packet) => self.datagrams.try_send_to(&packet, address).is_ok(),
            Err(_) => false,
        }
    }

    fn send_fast(&self, key: &Key, message: Message) {
        let Some(link) = self.links.get(key) else {
            return;
        };
        if !(link.datagrams_reach_peer && self.send_datagram(link, &message)) {
            let _ = link.outbox.send(message);
        }
    }

    async fn on_datagram(&mut self, from: SocketAddr, packet: &[u8]) {
        let Some((&key, link)) = self.links.iter_mut().find(|(_, link)| link.datagram_address == Some(from)) else {
            return;
        };
        let Some(message) = link.channel.open_datagram(packet) else {
            return;
        };
        link.heard_at = Instant::now();
        if !link.datagrams_reach_us {
            link.datagrams_reach_us = true;
            let _ = link.outbox.send(Message::DatagramsArrive);
        }
        match message {
            Message::Ping(time) => {
                if let Some(link) = self.links.get(&key) {
                    self.send_datagram(link, &Message::Pong(time));
                }
            }
            other => self.on_message(key, other).await,
        }
    }

    async fn apply_motion(&mut self, (dx, dy): (f64, f64)) {
        let Some(handle) = self.controlled_by.map(|key| crate::store::handle(&key)) else {
            return;
        };
        if (dx, dy) == (0.0, 0.0) || screen::move_by(dx, dy) {
            return;
        }
        let motion = Event::Pointer(PointerEvent::Motion { time: 0, dx, dy });
        self.emulate(motion, handle).await;
    }

    /// Never lets the input path wait on the emulation for long: a stuck
    /// emulation call must not take the whole engine down with it.
    async fn emulate(&mut self, event: Event, handle: u64) {
        match timeout(EMULATION_TIMEOUT, self.emulation.consume(event, handle)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.log.write(&format!("emulation failed: {error}")),
            Err(_) => self.log.write("emulation timed out"),
        }
    }

    async fn release_emulated_keys(&mut self, handle: u64) {
        if timeout(EMULATION_TIMEOUT, self.emulation.release_keys(handle)).await.is_err() {
            self.log.write("releasing emulated keys timed out");
        }
    }

    fn scaled_from(&self, peer: &Peer, dx: f64, dy: f64) -> (f64, f64) {
        let Some(theirs) = &peer.screen else {
            return (dx, dy);
        };
        let ours = &self.screen;
        let horizontal = (theirs.width_mm / theirs.width_px) / (ours.width_mm / ours.width_px);
        let vertical = (theirs.height_mm / theirs.height_px) / (ours.height_mm / ours.height_px);
        (dx * horizontal as f64, dy * vertical as f64)
    }

    async fn track(&mut self, peer: &Peer) {
        if let Err(error) = self.capture.create(peer.handle(), peer.side.position()).await {
            eprintln!("cannot watch the edge toward {}: {error}", peer.name);
        }
        self.emulation.create(peer.handle()).await;
    }

    async fn untrack(&mut self, peer: &Peer) {
        let _ = self.capture.destroy(peer.handle()).await;
        self.emulation.destroy(peer.handle()).await;
    }

    async fn on_capture(&mut self, handle: CaptureHandle, event: CaptureEvent) {
        let Some(peer) = self.store.peers.iter().find(|peer| peer.handle() == handle).cloned() else {
            let _ = self.capture.release().await;
            return;
        };
        match event {
            CaptureEvent::Begin => {
                let landing = self.landing_on(&peer);
                match landing {
                    Some(y) if self.controlled_by == Some(peer.key) => {
                        self.log.write("edge reached while controlled, handing back");
                        self.controlled_by = None;
                        let _ = self.capture.release().await;
                        self.release_emulated_keys(handle).await;
                        self.send(&peer.key, Message::Leave { y });
                        self.share_clipboard(peer.key);
                    }
                    Some(y) if self.links.contains_key(&peer.key) => {
                        self.log.write("edge reached, taking control");
                        self.controlling = Some(peer.key);
                        self.send(&peer.key, Message::Enter { y });
                        self.share_clipboard(peer.key);
                    }
                    _ => {
                        self.log.write("edge reached outside the overlap or offline");
                        let _ = self.capture.release().await;
                    }
                }
                self.refresh();
            }
            CaptureEvent::Input(_) if self.controlling != Some(peer.key) => {
                self.log.write("captured input with no peer under control, releasing");
                self.release().await;
            }
            CaptureEvent::Input(event) => {
                {
                    match event {
                        Event::Pointer(PointerEvent::Motion { dx, dy, .. }) => self.queue_motion(peer.key, dx, dy),
                        _ => {
                            self.flush_motion();
                            self.send(&peer.key, Message::Input(event));
                        }
                    }
                }
                self.release_at = match (self.capture.keys_pressed(&RELEASE_CHORD), self.release_at) {
                    (true, None) => Some(Instant::now() + RELEASE_HOLD),
                    (true, pending) => pending,
                    (false, _) => None,
                };
            }
        }
    }

    /// A 1000 Hz mouse would put a packet on the air every millisecond; merging
    /// motion into one packet per interval keeps Wi-Fi from queueing them.
    fn queue_motion(&mut self, key: Key, dx: f64, dy: f64) {
        let (queued_dx, queued_dy) = self.pending_motion.map_or((0.0, 0.0), |(_, x, y)| (x, y));
        self.pending_motion = Some((key, queued_dx + dx, queued_dy + dy));
        let due = self.motion_sent_at + MOTION_INTERVAL;
        if Instant::now() >= due {
            self.flush_motion();
        } else {
            self.motion_due = Some(due);
        }
    }

    fn flush_motion(&mut self) {
        self.motion_due = None;
        let Some((key, dx, dy)) = self.pending_motion.take() else {
            return;
        };
        self.motion_sent_at = Instant::now();
        self.sent_motion.record();
        self.send_fast(&key, Message::Input(Event::Pointer(PointerEvent::Motion { time: 0, dx, dy })));
    }

    /// Always lets go of the local input, even when the engine believes
    /// nothing is captured: a stale capture must never keep the mouse.
    async fn release(&mut self) {
        self.flush_motion();
        self.release_at = None;
        let pressed = self.capture.take_pressed_keys();
        if let Err(error) = self.capture.release().await {
            self.log.write(&format!("capture release failed: {error}"));
        }
        let Some(key) = self.controlling.take() else {
            return;
        };
        self.log.write(&format!("released control, {} key(s) still down", pressed.len()));
        for key_code in pressed {
            let up = KeyboardEvent::Key { time: 0, key: key_code as u32, state: 0 };
            self.send(&key, Message::Input(Event::Keyboard(up)));
        }
        let y = self.store.peer(&key).and_then(|peer| self.landing_on(peer)).unwrap_or(0.5);
        self.send(&key, Message::Leave { y });
        self.refresh();
    }

    fn install_wifi_fix(&self) {
        let staging = self.data_dir.clone();
        let signals = self.signals.clone();
        spawn_local(async move {
            let prompt = crate::texts::get().wifi_fix_prompt;
            if let Ok(outcome) = tokio::task::spawn_blocking(move || wifi::install_helper(&staging, prompt)).await {
                let _ = signals.send(Signal::WifiFix(outcome));
            }
        });
    }

    fn wifi_fix_done(&mut self, outcome: wifi::Outcome) {
        match outcome {
            wifi::Outcome::Installed => {
                self.log.write("wifi fix installed");
                self.wifi_fix_installed = true;
            }
            wifi::Outcome::Declined => {
                self.store.settings.wifi_fix_declined = true;
                if let Err(error) = self.store.save_settings() {
                    self.log.write(&format!("cannot save settings: {error}"));
                }
            }
            wifi::Outcome::Failed(error) => self.log.write(&format!("wifi fix failed: {error}")),
        }
        self.refresh();
    }

    fn share_clipboard(&self, key: Key) {
        let known = self.clipboard_known;
        let signals = self.signals.clone();
        spawn_local(async move {
            if let Ok(Some(snapshot)) = tokio::task::spawn_blocking(move || clipboard::read_changed(known)).await {
                let _ = signals.send(Signal::ClipboardRead(key, snapshot));
            }
        });
    }

    fn send_clipboard(&mut self, key: Key, snapshot: clipboard::Snapshot) {
        if self.clipboard_known == Some(snapshot.fingerprint) || snapshot.encoded.len() > CLIPBOARD_LIMIT as usize {
            return;
        }
        self.clipboard_known = Some(snapshot.fingerprint);
        let total = snapshot.encoded.len() as u32;
        for (index, chunk) in snapshot.encoded.chunks(wire::CLIPBOARD_CHUNK).enumerate() {
            let offset = (index * wire::CLIPBOARD_CHUNK) as u32;
            self.send(&key, Message::Clipboard { total, offset, data: chunk.to_vec() });
        }
        self.log.write(&format!("clipboard sent, {total} bytes"));
    }

    fn receive_clipboard(&mut self, key: Key, total: u32, offset: u32, data: Vec<u8>) {
        let Some(link) = self.links.get_mut(&key) else {
            return;
        };
        if offset == 0 {
            link.clipboard_incoming.clear();
        }
        if total > CLIPBOARD_LIMIT || offset as usize != link.clipboard_incoming.len() {
            link.clipboard_incoming.clear();
            return;
        }
        link.clipboard_incoming.extend(data);
        if link.clipboard_incoming.len() < total as usize {
            return;
        }
        let encoded = std::mem::take(&mut link.clipboard_incoming);
        self.log.write(&format!("clipboard received, {total} bytes"));
        let signals = self.signals.clone();
        spawn_local(async move {
            let written = tokio::task::spawn_blocking(move || clipboard::write(&encoded).map_err(|error| error.to_string())).await;
            if let Ok(result) = written {
                let _ = signals.send(Signal::ClipboardWritten(result));
            }
        });
    }

    fn landing_on(&self, peer: &Peer) -> Option<f32> {
        let fraction = screen::edge_fraction(peer.side);
        match &peer.screen {
            Some(theirs) => layout::crossing(&self.screen, theirs, peer.side, peer.offset_mm, fraction),
            None => Some(fraction),
        }
    }

    fn apply_screen_override(&mut self) {
        let detected = screen::detect();
        self.screen = match self.store.settings.screen_mm {
            Some((width_mm, height_mm)) => Screen { width_mm, height_mm, ..detected },
            None => detected,
        };
    }

    fn broadcast(&self, message: Message) {
        for link in self.links.values() {
            let _ = link.outbox.send(message.clone());
        }
    }

    async fn place(&mut self, key: Key, side: Side, offset_mm: f32) {
        let Some(peer) = self.store.peer(&key).cloned() else {
            return;
        };
        if peer.side != side {
            self.untrack(&peer).await;
        }
        if let Some(known) = self.store.peer_mut(&key) {
            known.side = side;
            known.offset_mm = offset_mm;
        }
        if peer.side != side {
            let moved = Peer { side, offset_mm, ..peer };
            self.track(&moved).await;
        }
        self.save_peers();
        self.ui.run(ui::layout_changed);
    }

    fn save_peers(&self) {
        if let Err(error) = self.store.save_peers() {
            eprintln!("cannot save the layout: {error}");
        }
    }

    fn layout_state(&self) -> serde_json::Value {
        let peers: Vec<_> = self
            .store
            .peers
            .iter()
            .map(|peer| {
                serde_json::json!({
                    "key": hex::encode(peer.key),
                    "name": peer.name,
                    "online": self.links.contains_key(&peer.key),
                    "screen": peer.screen,
                    "side": peer.side,
                    "offsetMm": peer.offset_mm,
                })
            })
            .collect();
        serde_json::json!({
            "me": { "name": self.name, "screen": self.screen, "detected": screen::detect() },
            "peers": peers,
        })
    }

    async fn on_message(&mut self, key: Key, message: Message) {
        let Some(peer) = self.store.peer(&key).cloned() else {
            return;
        };
        match message {
            Message::Enter { y } => {
                self.log.write("peer took control");
                self.controlled_by = Some(key);
                screen::warp_to_edge(peer.side, y);
                self.refresh();
            }
            Message::Input(Event::Pointer(PointerEvent::Motion { dx, dy, .. })) => {
                if self.controlled_by == Some(key) {
                    self.received_motion.record();
                    let (dx, dy) = self.scaled_from(&peer, dx, dy);
                    self.apply_motion((dx, dy)).await;
                }
            }
            Message::Input(event) => {
                if self.controlled_by == Some(key) {
                    self.emulate(event, peer.handle()).await;
                }
            }
            Message::Leave { y } => {
                if self.controlling == Some(key) {
                    self.log.write("peer handed control back");
                    self.release().await;
                    screen::warp_to_edge(peer.side, y);
                }
                if self.controlled_by == Some(key) {
                    self.log.write("peer took its input back");
                    self.controlled_by = None;
                    self.release_emulated_keys(peer.handle()).await;
                    self.share_clipboard(key);
                    self.refresh();
                }
            }
            Message::Clipboard { total, offset, data } => self.receive_clipboard(key, total, offset, data),
            Message::Ping(time) => self.send(&key, Message::Pong(time)),
            Message::Pong(time) => {
                let latency = self.now().saturating_sub(time) / 2;
                if let Some(link) = self.links.get_mut(&key) {
                    link.latency_us = Some(latency);
                }
                self.refresh();
            }
            Message::Screen(theirs) => {
                if let Some(known) = self.store.peer_mut(&key) {
                    if known.screen != Some(theirs) {
                        known.screen = Some(theirs);
                        self.save_peers();
                        self.ui.run(ui::layout_changed);
                    }
                }
            }
            Message::Arrange { side, offset_mm } => self.place(key, side, offset_mm).await,
            Message::Resize { width_mm, height_mm } => {
                self.store.settings.screen_mm = (width_mm > 0.0 && height_mm > 0.0).then_some((width_mm, height_mm));
                self.resized();
            }
            Message::Identify(number) => {
                self.ui.run(move |app| ui::identify(app, number));
                self.close_identify_later();
            }
            Message::DatagramPort(port) => {
                if let Some(link) = self.links.get_mut(&key) {
                    link.datagram_address = Some(SocketAddr::new(link.ip, port));
                }
            }
            Message::DatagramsArrive => {
                if let Some(link) = self.links.get_mut(&key) {
                    link.datagrams_reach_peer = true;
                }
            }
        }
    }

    fn resized(&mut self) {
        if let Err(error) = self.store.save_settings() {
            eprintln!("cannot save the screen size: {error}");
        }
        self.apply_screen_override();
        self.broadcast(Message::Screen(self.screen));
        self.ui.run(ui::layout_changed);
    }

    fn close_identify_later(&self) {
        let ui = self.ui.clone();
        spawn_local(async move {
            sleep(IDENTIFY_DISPLAY).await;
            ui.run(ui::close_identify);
        });
    }

    async fn on_signal(&mut self, signal: Signal) {
        match signal {
            Signal::Incoming(stream) => {
                let known = self.store.peers.iter().map(|peer| peer.key).collect();
                spawn_local(triage(stream, self.store.private, known, self.signals.clone()));
            }
            Signal::PairRequest(stream, name) => self.answer_pairing(stream, name),
            Signal::Session(key, stream, transport) => {
                self.connecting.remove(&key);
                self.attach(key, stream, transport);
                self.refresh();
            }
            Signal::ConnectFailed(key) => {
                self.connecting.remove(&key);
            }
            Signal::Paired(peer) => {
                self.pairing = None;
                if let Some(known) = self.store.peer(&peer.key).cloned() {
                    self.untrack(&known).await;
                }
                if let Err(error) = self.store.add(peer.clone()) {
                    eprintln!("cannot save {}: {error}", peer.name);
                }
                self.track(&peer).await;
                self.ui.run(ui::close_pairing);
                self.refresh();
            }
            Signal::PairingFailed => {
                self.pairing = None;
                self.code_entry = None;
                self.ui.run(ui::pairing_failed);
                let ui = self.ui.clone();
                spawn_local(async move {
                    sleep(FAILURE_DISPLAY).await;
                    ui.run(ui::close_pairing);
                });
            }
            Signal::Received(key, message) => {
                if let Some(link) = self.links.get_mut(&key) {
                    link.heard_at = Instant::now();
                }
                self.on_message(key, message).await;
            }
            Signal::Closed(key, id) => {
                if self.links.get(&key).is_some_and(|link| link.id == id) {
                    self.log.write("link closed");
                    self.drop_link(key).await;
                }
            }
            Signal::ClipboardRead(key, snapshot) => self.send_clipboard(key, snapshot),
            Signal::ClipboardWritten(Ok(fingerprint)) => self.clipboard_known = fingerprint,
            Signal::ClipboardWritten(Err(error)) => self.log.write(&format!("clipboard write failed: {error}")),
            Signal::WifiFix(outcome) => self.wifi_fix_done(outcome),
        }
    }

    async fn drop_link(&mut self, key: Key) {
        if self.controlling == Some(key) {
            self.release().await;
        }
        self.links.remove(&key);
        if self.controlled_by == Some(key) {
            self.controlled_by = None;
            self.release_emulated_keys(crate::store::handle(&key)).await;
        }
        self.refresh();
        self.ui.run(ui::layout_changed);
    }

    fn attach(&mut self, key: Key, stream: TcpStream, transport: StatelessTransportState) {
        let Ok(peer_address) = stream.peer_addr() else {
            return;
        };
        let (mut receiver, mut sender, channel) = wire::split(stream, transport);
        self.log.write("link up");
        let (outbox, mut queued) = mpsc::unbounded_channel();
        let id = self.next_link;
        self.next_link += 1;
        spawn_local(async move {
            while let Some(message) = queued.recv().await {
                if sender.send(message).await.is_err() {
                    break;
                }
            }
        });
        let signals = self.signals.clone();
        spawn_local(async move {
            while let Ok(message) = receiver.receive().await {
                if signals.send(Signal::Received(key, message)).is_err() {
                    return;
                }
            }
            let _ = signals.send(Signal::Closed(key, id));
        });
        let _ = outbox.send(Message::Screen(self.screen));
        if let Ok(local) = self.datagrams.local_addr() {
            let _ = outbox.send(Message::DatagramPort(local.port()));
        }
        self.links.insert(
            key,
            Link {
                id,
                outbox,
                channel,
                ip: peer_address.ip(),
                datagram_address: None,
                datagrams_reach_peer: false,
                datagrams_reach_us: false,
                latency_us: None,
                heard_at: Instant::now(),
                clipboard_incoming: Vec::new(),
            },
        );
        self.ui.run(ui::layout_changed);
    }

    fn answer_pairing(&mut self, mut stream: TcpStream, name: String) {
        if self.pairing.is_some() {
            return;
        }
        let code = format!("{:06}", rand::random_range(0..1_000_000));
        let (shown_code, shown_name) = (code.clone(), name.clone());
        self.ui.run(move |app| ui::show_code(app, &shown_code, &shown_name));
        let private = self.store.private;
        let signals = self.signals.clone();
        self.pairing = Some(spawn_local(async move {
            let paired = timeout(PAIRING_TIMEOUT, wire::pair_as_responder(&mut stream, &private, &code)).await;
            let _ = signals.send(match paired {
                Ok(Ok(key)) => Signal::Paired(Peer { name, key, side: Side::Left, offset_mm: 0.0, screen: None }),
                _ => Signal::PairingFailed,
            });
        }));
    }

    fn start_pairing(&mut self, key: Key) {
        let Some(nearby) = self.nearby.get(&key) else {
            return;
        };
        if self.pairing.is_some() {
            return;
        }
        let asked_name = nearby.name.clone();
        self.ui.run(move |app| ui::ask_code(app, &asked_name));
        let (code_entry, code) = oneshot::channel::<String>();
        self.code_entry = Some(code_entry);
        let (addr, name, own_name, private) = (nearby.addr, nearby.name.clone(), self.name.clone(), self.store.private);
        let signals = self.signals.clone();
        self.pairing = Some(spawn_local(async move {
            let paired = timeout(PAIRING_TIMEOUT, async {
                let mut stream = TcpStream::connect(addr).await?;
                wire::request_pairing(&mut stream, &own_name).await?;
                let code = code.await?;
                let paired = wire::pair_as_initiator(&mut stream, &private, &code).await?;
                if paired != key {
                    return Err::<(), Error>("a different device answered".into());
                }
                Ok(())
            })
            .await;
            let _ = signals.send(match paired {
                Ok(Ok(())) => Signal::Paired(Peer { name, key, side: Side::Right, offset_mm: 0.0, screen: None }),
                _ => Signal::PairingFailed,
            });
        }));
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::Layout(reply) => {
                let _ = reply.send(self.layout_state());
            }
            Command::Arrange { key, side, offset_mm } => {
                self.place(key, side, offset_mm).await;
                self.send(&key, Message::Arrange { side: side.opposite(), offset_mm: -offset_mm });
            }
            Command::Resize { key: None, size_mm } => {
                self.store.settings.screen_mm = size_mm;
                self.resized();
            }
            Command::Resize { key: Some(key), size_mm } => {
                let (width_mm, height_mm) = size_mm.unwrap_or((0.0, 0.0));
                self.send(&key, Message::Resize { width_mm, height_mm });
            }
            Command::InstallWifiFix => self.install_wifi_fix(),
            Command::Identify => {
                self.ui.run(|app| ui::identify(app, 1));
                let online = self.store.peers.iter().filter(|peer| self.links.contains_key(&peer.key));
                for (number, peer) in (2..).zip(online) {
                    self.send(&peer.key, Message::Identify(number));
                }
                self.close_identify_later();
            }
            Command::Pair(key) => self.start_pairing(key),
            Command::SubmitCode(code) => {
                if let Some(entry) = self.code_entry.take() {
                    let _ = entry.send(code);
                }
            }
            Command::CancelPairing => {
                if let Some(pairing) = self.pairing.take() {
                    pairing.abort();
                }
                self.code_entry = None;
            }
        }
    }

    fn on_discovery(&mut self, event: mdns_sd::ServiceEvent) {
        match discovery::interpret(event, &self.store.public) {
            Some(Discovery::Found { service, key, name, addr }) => {
                self.nearby.insert(key, Nearby { name, addr, service });
            }
            Some(Discovery::Lost { service }) => self.nearby.retain(|_, nearby| nearby.service != service),
            None => return,
        }
        self.refresh();
    }

    async fn on_tick(&mut self) {
        let silent: Vec<Key> = self
            .links
            .iter()
            .filter(|(_, link)| link.heard_at.elapsed() > SILENCE_LIMIT)
            .map(|(key, _)| *key)
            .collect();
        for key in silent {
            self.log.write("peer went silent, link dropped");
            self.drop_link(key).await;
        }
        self.write_diagnostics();
        let now = self.now();
        for link in self.links.values() {
            let probed = self.send_datagram(link, &Message::Ping(now));
            if !(probed && link.datagrams_reach_peer) {
                let _ = link.outbox.send(Message::Ping(now));
            }
        }
        for peer in &self.store.peers {
            let reachable = self.nearby.get(&peer.key);
            let initiates = self.store.public < peer.key;
            if let (Some(nearby), true) = (reachable, initiates) {
                if !self.links.contains_key(&peer.key) && self.connecting.insert(peer.key) {
                    spawn_local(connect(nearby.addr, self.store.private, peer.key, self.signals.clone()));
                }
            }
        }
    }

    fn write_diagnostics(&mut self) {
        let link = self.controlling.or(self.controlled_by).and_then(|key| self.links.get(&key));
        let transport = match link {
            Some(link) if link.datagrams_reach_peer => "udp",
            Some(_) => "tcp",
            None => "none",
        };
        let latency = link.and_then(|link| link.latency_us).unwrap_or(0);
        if let Some(summary) = self.sent_motion.summary() {
            self.log.write(&format!("sent {summary} via {transport} latency {latency}us"));
        }
        if let Some(summary) = self.received_motion.summary() {
            self.log.write(&format!("received {summary} latency {latency}us"));
        }
    }

    fn refresh(&mut self) {
        let status = if self.controlling.is_some() || self.controlled_by.is_some() {
            Status::Active
        } else {
            Status::Ready
        };
        let peers = self
            .store
            .peers
            .iter()
            .map(|peer| PeerView {
                name: peer.name.clone(),
                latency_ms: self.links.get(&peer.key).map(|link| link.latency_us.map(|us| us / 1000)),
            })
            .collect();
        let nearby = self
            .nearby
            .iter()
            .filter(|(key, _)| self.store.peer(key).is_none())
            .map(|(key, nearby)| (*key, nearby.name.clone()))
            .collect();
        let view = View { status, peers, nearby, missing: Vec::new(), offer_wifi_fix: !self.wifi_fix_installed };
        if self.shown.as_ref() != Some(&view) {
            let shown = view.clone();
            self.ui.run(move |app| ui::refresh(app, &shown));
            self.shown = Some(view);
        }
    }
}
