use std::{fs, io, path::PathBuf};

use input_capture::Position;
use serde::{Deserialize, Serialize};

use crate::{
    screen::Screen,
    wire::{Error, Key, SESSION_PATTERN},
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    pub fn position(self) -> Position {
        match self {
            Side::Left => Position::Left,
            Side::Right => Position::Right,
            Side::Top => Position::Top,
            Side::Bottom => Position::Bottom,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    #[serde(with = "hex::serde")]
    pub key: Key,
    pub side: Side,
    #[serde(default)]
    pub offset_mm: f32,
    #[serde(default)]
    pub screen: Option<Screen>,
}

impl Peer {
    pub fn handle(&self) -> u64 {
        handle(&self.key)
    }
}

pub fn handle(key: &Key) -> u64 {
    u64::from_be_bytes(key[..8].try_into().expect("8 bytes"))
}

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    pub screen_mm: Option<(f32, f32)>,
    #[serde(default)]
    pub wifi_fix_declined: bool,
}

pub struct Store {
    dir: PathBuf,
    pub private: Key,
    pub public: Key,
    pub peers: Vec<Peer>,
    pub settings: Settings,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: PathBuf) -> T {
    fs::read(path)
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}

impl Store {
    pub fn load(dir: PathBuf) -> Result<Self, Error> {
        fs::create_dir_all(&dir)?;
        let identity = dir.join("identity");
        let bytes = match fs::read(&identity) {
            Ok(bytes) if bytes.len() == 64 => bytes,
            _ => {
                let pair = snow::Builder::new(SESSION_PATTERN.parse()?).generate_keypair()?;
                let bytes = [pair.private, pair.public].concat();
                write_private(&identity, &bytes)?;
                bytes
            }
        };
        Ok(Self {
            private: bytes[..32].try_into()?,
            public: bytes[32..].try_into()?,
            peers: read_json(dir.join("peers.json")),
            settings: read_json(dir.join("settings.json")),
            dir,
        })
    }

    pub fn peer(&self, key: &Key) -> Option<&Peer> {
        self.peers.iter().find(|peer| &peer.key == key)
    }

    pub fn peer_mut(&mut self, key: &Key) -> Option<&mut Peer> {
        self.peers.iter_mut().find(|peer| &peer.key == key)
    }

    pub fn add(&mut self, peer: Peer) -> io::Result<()> {
        self.peers.retain(|known| known.key != peer.key);
        self.peers.push(peer);
        self.save_peers()
    }

    pub fn save_peers(&self) -> io::Result<()> {
        fs::write(self.dir.join("peers.json"), serde_json::to_vec_pretty(&self.peers)?)
    }

    pub fn save_settings(&self) -> io::Result<()> {
        fs::write(self.dir.join("settings.json"), serde_json::to_vec_pretty(&self.settings)?)
    }
}

#[cfg(unix)]
fn write_private(path: &PathBuf, bytes: &[u8]) -> io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &PathBuf, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}
