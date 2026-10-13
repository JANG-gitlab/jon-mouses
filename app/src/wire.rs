use std::{cell::Cell, error::Error as StdError, rc::Rc};

use input_event::Event;
use lan_mouse_proto::{MAX_EVENT_SIZE, ProtoEvent};
use snow::{Builder, HandshakeState, StatelessTransportState};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
};

use crate::{screen::Screen, store::Side};

pub type Key = [u8; 32];
pub type Error = Box<dyn StdError>;

pub const SESSION_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
const PAIRING_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
const PAIRING_CONTEXT: &[u8] = b"jon-mouses pairing v1";
const TAG_LEN: usize = 16;

pub const PAIR_REQUEST: u8 = b'P';
pub const SESSION_REQUEST: u8 = b'S';

pub const CLIPBOARD_CHUNK: usize = 60_000;

#[derive(Clone, Debug)]
pub enum Message {
    Enter { y: f32 },
    Leave { y: f32 },
    Input(Event),
    Ping(u64),
    Pong(u64),
    Screen(Screen),
    Arrange { side: Side, offset_mm: f32 },
    Resize { width_mm: f32, height_mm: f32 },
    Identify(u8),
    DatagramPort(u16),
    DatagramsArrive,
    Clipboard { total: u32, offset: u32, data: Vec<u8> },
}

fn floats<const N: usize>(body: &[u8]) -> Option<[f32; N]> {
    if body.len() != N * 4 {
        return None;
    }
    let mut values = [0.0; N];
    for (value, bytes) in values.iter_mut().zip(body.chunks_exact(4)) {
        *value = f32::from_be_bytes(bytes.try_into().ok()?);
    }
    Some(values)
}

impl Message {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + MAX_EVENT_SIZE);
        match *self {
            Message::Enter { y } => {
                out.push(0);
                out.extend(y.to_be_bytes());
            }
            Message::Leave { y } => {
                out.push(1);
                out.extend(y.to_be_bytes());
            }
            Message::Input(event) => {
                let (buf, len): ([u8; MAX_EVENT_SIZE], usize) = ProtoEvent::Input(event).into();
                out.push(2);
                out.extend(&buf[..len]);
            }
            Message::Ping(time) => {
                out.push(3);
                out.extend(time.to_be_bytes());
            }
            Message::Pong(time) => {
                out.push(4);
                out.extend(time.to_be_bytes());
            }
            Message::Screen(screen) => {
                out.push(5);
                for value in [screen.width_px, screen.height_px, screen.width_mm, screen.height_mm] {
                    out.extend(value.to_be_bytes());
                }
            }
            Message::Clipboard { total, offset, ref data } => {
                out.push(11);
                out.extend(total.to_be_bytes());
                out.extend(offset.to_be_bytes());
                out.extend(data);
            }
            Message::Arrange { side, offset_mm } => {
                out.push(6);
                out.push(Side::ALL.iter().position(|known| *known == side).expect("known side") as u8);
                out.extend(offset_mm.to_be_bytes());
            }
            Message::Resize { width_mm, height_mm } => {
                out.push(7);
                out.extend(width_mm.to_be_bytes());
                out.extend(height_mm.to_be_bytes());
            }
            Message::Identify(number) => {
                out.push(8);
                out.push(number);
            }
            Message::DatagramPort(port) => {
                out.push(9);
                out.extend(port.to_be_bytes());
            }
            Message::DatagramsArrive => out.push(10),
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (&tag, body) = bytes.split_first()?;
        Some(match tag {
            0 => Message::Enter { y: f32::from_be_bytes(body.try_into().ok()?) },
            1 => Message::Leave { y: f32::from_be_bytes(body.try_into().ok()?) },
            2 => {
                let mut buf = [0u8; MAX_EVENT_SIZE];
                buf.get_mut(..body.len())?.copy_from_slice(body);
                match ProtoEvent::try_from(buf).ok()? {
                    ProtoEvent::Input(event) => Message::Input(event),
                    _ => return None,
                }
            }
            3 => Message::Ping(u64::from_be_bytes(body.try_into().ok()?)),
            4 => Message::Pong(u64::from_be_bytes(body.try_into().ok()?)),
            5 => {
                let [width_px, height_px, width_mm, height_mm] = floats(body)?;
                Message::Screen(Screen { width_px, height_px, width_mm, height_mm })
            }
            6 => {
                let (&side, offset) = body.split_first()?;
                let [offset_mm] = floats(offset)?;
                Message::Arrange { side: *Side::ALL.get(side as usize)?, offset_mm }
            }
            7 => {
                let [width_mm, height_mm] = floats(body)?;
                Message::Resize { width_mm, height_mm }
            }
            8 => Message::Identify(*body.first()?),
            9 => Message::DatagramPort(u16::from_be_bytes(body.try_into().ok()?)),
            10 => Message::DatagramsArrive,
            11 => {
                let (header, data) = body.split_at_checked(8)?;
                Message::Clipboard {
                    total: u32::from_be_bytes(header[..4].try_into().ok()?),
                    offset: u32::from_be_bytes(header[4..].try_into().ok()?),
                    data: data.to_vec(),
                }
            }
            _ => return None,
        })
    }
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), payload: &[u8]) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(2 + payload.len());
    frame.extend((payload.len() as u16).to_be_bytes());
    frame.extend(payload);
    writer.write_all(&frame).await
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let len = reader.read_u16().await? as usize;
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

async fn send_handshake(stream: &mut TcpStream, state: &mut HandshakeState) -> Result<(), Error> {
    let mut buf = vec![0; u16::MAX as usize];
    let len = state.write_message(&[], &mut buf)?;
    Ok(write_frame(stream, &buf[..len]).await?)
}

async fn receive_handshake(stream: &mut TcpStream, state: &mut HandshakeState) -> Result<(), Error> {
    let frame = read_frame(stream).await?;
    let mut buf = vec![0; frame.len()];
    state.read_message(&frame, &mut buf)?;
    Ok(())
}

fn remote_key(state: &HandshakeState) -> Result<Key, Error> {
    let key = state.get_remote_static().ok_or("peer sent no static key")?;
    Ok(key.try_into()?)
}

async fn shared_secret(stream: &mut TcpStream, code: &str) -> Result<Key, Error> {
    let (spake, outbound) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(code.as_bytes()),
        &Identity::new(PAIRING_CONTEXT),
    );
    write_frame(stream, &outbound).await?;
    let inbound = read_frame(stream).await?;
    let secret = spake.finish(&inbound).map_err(|error| format!("{error:?}"))?;
    secret.try_into().map_err(|_| "unexpected secret size".into())
}

pub async fn request_pairing(stream: &mut TcpStream, name: &str) -> Result<(), Error> {
    stream.write_all(&[PAIR_REQUEST]).await?;
    Ok(write_frame(stream, name.as_bytes()).await?)
}

pub async fn read_pairing_name(stream: &mut TcpStream) -> Result<String, Error> {
    let name = read_frame(stream).await?;
    Ok(String::from_utf8_lossy(&name).chars().take(64).collect())
}

pub async fn pair_as_initiator(stream: &mut TcpStream, private: &Key, code: &str) -> Result<Key, Error> {
    let secret = shared_secret(stream, code).await?;
    let mut state = Builder::new(PAIRING_PATTERN.parse()?)
        .local_private_key(private)?
        .psk(3, &secret)?
        .build_initiator()?;
    send_handshake(stream, &mut state).await?;
    receive_handshake(stream, &mut state).await?;
    send_handshake(stream, &mut state).await?;
    let peer = remote_key(&state)?;
    let mut transport = state.into_transport_mode()?;
    let confirmation = read_frame(stream).await?;
    let mut buf = vec![0; confirmation.len()];
    transport.read_message(&confirmation, &mut buf)?;
    Ok(peer)
}

pub async fn pair_as_responder(stream: &mut TcpStream, private: &Key, code: &str) -> Result<Key, Error> {
    let secret = shared_secret(stream, code).await?;
    let mut state = Builder::new(PAIRING_PATTERN.parse()?)
        .local_private_key(private)?
        .psk(3, &secret)?
        .build_responder()?;
    receive_handshake(stream, &mut state).await?;
    send_handshake(stream, &mut state).await?;
    receive_handshake(stream, &mut state).await?;
    let peer = remote_key(&state)?;
    let mut transport = state.into_transport_mode()?;
    let mut buf = vec![0; TAG_LEN];
    let len = transport.write_message(&[], &mut buf)?;
    write_frame(stream, &buf[..len]).await?;
    Ok(peer)
}

pub async fn open_session(mut stream: TcpStream, private: &Key, peer: &Key) -> Result<(TcpStream, StatelessTransportState), Error> {
    stream.write_all(&[SESSION_REQUEST]).await?;
    let mut state = Builder::new(SESSION_PATTERN.parse()?)
        .local_private_key(private)?
        .remote_public_key(peer)?
        .build_initiator()?;
    send_handshake(&mut stream, &mut state).await?;
    receive_handshake(&mut stream, &mut state).await?;
    Ok((stream, state.into_stateless_transport_mode()?))
}

pub async fn accept_session(mut stream: TcpStream, private: &Key, known: &[Key]) -> Result<(Key, TcpStream, StatelessTransportState), Error> {
    let mut state = Builder::new(SESSION_PATTERN.parse()?)
        .local_private_key(private)?
        .build_responder()?;
    receive_handshake(&mut stream, &mut state).await?;
    let peer = remote_key(&state)?;
    if !known.contains(&peer) {
        return Err("unknown peer".into());
    }
    send_handshake(&mut stream, &mut state).await?;
    Ok((peer, stream, state.into_stateless_transport_mode()?))
}

/// One Noise session shared by the stream and the datagrams: the stream uses
/// even nonces in order, datagrams carry odd nonces so late ones can be dropped.
pub struct Channel {
    transport: StatelessTransportState,
    sent: Cell<u64>,
    received: Cell<u64>,
    datagrams_sent: Cell<u64>,
    latest_datagram: Cell<Option<u64>>,
}

impl Channel {
    fn seal(&self, nonce: u64, message: &Message) -> Result<Vec<u8>, Error> {
        let plain = message.encode();
        let mut sealed = vec![0; plain.len() + TAG_LEN];
        let len = self.transport.write_message(nonce, &plain, &mut sealed)?;
        sealed.truncate(len);
        Ok(sealed)
    }

    fn open(&self, nonce: u64, sealed: &[u8]) -> Result<Message, Error> {
        let mut plain = vec![0; sealed.len()];
        let len = self.transport.read_message(nonce, sealed, &mut plain)?;
        Message::decode(&plain[..len]).ok_or_else(|| "malformed message".into())
    }

    pub fn seal_datagram(&self, message: &Message) -> Result<Vec<u8>, Error> {
        let count = self.datagrams_sent.get();
        self.datagrams_sent.set(count + 1);
        let nonce = count * 2 + 1;
        let mut packet = nonce.to_be_bytes().to_vec();
        packet.extend(self.seal(nonce, message)?);
        Ok(packet)
    }

    pub fn open_datagram(&self, packet: &[u8]) -> Option<Message> {
        let (nonce, sealed) = packet.split_at_checked(8)?;
        let nonce = u64::from_be_bytes(nonce.try_into().ok()?);
        if nonce % 2 == 0 || self.latest_datagram.get().is_some_and(|latest| nonce <= latest) {
            return None;
        }
        let message = self.open(nonce, sealed).ok()?;
        self.latest_datagram.set(Some(nonce));
        Some(message)
    }
}

pub struct Receiver {
    read: OwnedReadHalf,
    channel: Rc<Channel>,
}

pub struct Sender {
    write: OwnedWriteHalf,
    channel: Rc<Channel>,
}

pub fn split(stream: TcpStream, transport: StatelessTransportState) -> (Receiver, Sender, Rc<Channel>) {
    let (read, write) = stream.into_split();
    let channel = Rc::new(Channel {
        transport,
        sent: Cell::new(0),
        received: Cell::new(0),
        datagrams_sent: Cell::new(0),
        latest_datagram: Cell::new(None),
    });
    (
        Receiver { read, channel: channel.clone() },
        Sender { write, channel: channel.clone() },
        channel,
    )
}

impl Receiver {
    pub async fn receive(&mut self) -> Result<Message, Error> {
        let frame = read_frame(&mut self.read).await?;
        let count = self.channel.received.get();
        self.channel.received.set(count + 1);
        self.channel.open(count * 2, &frame)
    }
}

impl Sender {
    pub async fn send(&mut self, message: Message) -> Result<(), Error> {
        let count = self.channel.sent.get();
        self.channel.sent.set(count + 1);
        let sealed = self.channel.seal(count * 2, &message)?;
        Ok(write_frame(&mut self.write, &sealed).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_event::PointerEvent;
    use tokio::net::TcpListener;

    fn keypair() -> (Key, Key) {
        let pair = Builder::new(SESSION_PATTERN.parse().unwrap()).generate_keypair().unwrap();
        (pair.private.try_into().unwrap(), pair.public.try_into().unwrap())
    }

    async fn connected() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap());
        let (client, server) = tokio::join!(client, listener.accept());
        (client.unwrap(), server.unwrap().0)
    }

    async fn pair(initiator_code: &str, responder_code: &str) -> (Result<Key, Error>, Result<Key, Error>) {
        let (a_private, _) = keypair();
        let (b_private, _) = keypair();
        let (mut a, mut b) = connected().await;
        tokio::join!(
            async move { pair_as_initiator(&mut a, &a_private, initiator_code).await },
            async move { pair_as_responder(&mut b, &b_private, responder_code).await },
        )
    }

    #[tokio::test]
    async fn pairing_needs_the_same_code() {
        let (a, b) = pair("482913", "482913").await;
        assert!(a.is_ok() && b.is_ok());
        let (a, b) = pair("482913", "482914").await;
        assert!(a.is_err() && b.is_err());
    }

    #[tokio::test]
    async fn session_carries_messages_and_rejects_strangers() {
        let (a_private, a_public) = keypair();
        let (b_private, b_public) = keypair();

        let mut request = [0u8];
        let (a, mut b) = connected().await;
        let accept = async {
            b.read_exact(&mut request).await.unwrap();
            accept_session(b, &b_private, &[a_public]).await
        };
        let (opened, accepted) = tokio::join!(open_session(a, &a_private, &b_public), accept);
        let (stream_a, transport_a) = opened.unwrap();
        let (peer, stream_b, transport_b) = accepted.unwrap();
        assert_eq!(peer, a_public);

        let (_, mut sender, sending) = split(stream_a, transport_a);
        let (mut receiver, _, receiving) = split(stream_b, transport_b);
        let motion = Event::Pointer(PointerEvent::Motion { time: 7, dx: 3.5, dy: -2.0 });
        let screen = Screen { width_px: 1470.0, height_px: 956.0, width_mm: 300.5, height_mm: 195.0 };
        for message in [
            Message::Enter { y: 0.25 },
            Message::Input(motion),
            Message::Ping(42),
            Message::Screen(screen),
            Message::Arrange { side: Side::Bottom, offset_mm: -12.5 },
            Message::Resize { width_mm: 597.0, height_mm: 336.0 },
            Message::Identify(2),
            Message::DatagramPort(40123),
            Message::DatagramsArrive,
            Message::Clipboard { total: 5, offset: 2, data: vec![1, 2, 3] },
        ] {
            sender.send(message.clone()).await.unwrap();
            assert_eq!(format!("{:?}", receiver.receive().await.unwrap()), format!("{message:?}"));
        }

        let first = sending.seal_datagram(&Message::Ping(1)).unwrap();
        let second = sending.seal_datagram(&Message::Input(motion)).unwrap();
        sender.send(Message::Ping(3)).await.unwrap();
        assert!(matches!(receiving.open_datagram(&second), Some(Message::Input(_))));
        assert!(receiving.open_datagram(&first).is_none(), "late datagram must be dropped");
        assert!(receiving.open_datagram(&second).is_none(), "replayed datagram must be dropped");
        assert!(matches!(receiver.receive().await.unwrap(), Message::Ping(3)));
        let mut forged = sending.seal_datagram(&Message::Ping(4)).unwrap();
        *forged.last_mut().unwrap() ^= 1;
        assert!(receiving.open_datagram(&forged).is_none());

        let (stranger_private, _) = keypair();
        let (a, mut b) = connected().await;
        let accept = async {
            b.read_exact(&mut request).await.unwrap();
            accept_session(b, &b_private, &[a_public]).await
        };
        let (_, accepted) = tokio::join!(open_session(a, &stranger_private, &b_public), accept);
        assert!(accepted.is_err());
    }
}
