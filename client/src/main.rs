#![feature(maybe_uninit_uninit_array)]
use futures_util::{SinkExt, StreamExt};
use serde::*;
use socket2::*;
use std::collections::BTreeMap;
use std::net::*;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::*;

const CHECKSUM: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_MPEG_2);
type PeerSockets = Arc<Mutex<BTreeMap<i64, WebSocketStream<MaybeTlsStream<TcpStream>>>>>;

mod wm;

#[derive(Deserialize)]
struct Config {
	jwt: String,
}

#[derive(Clone, Deserialize)]
pub struct Group {
	users: Vec<(User, IpAddr)>,
}

#[derive(Clone, Deserialize)]
pub struct IpAddr {
	pub inner: std::net::IpAddr,
}

#[derive(Deserialize, Clone)]
pub struct User {
	pub id: i64,
	pub username: String,
	pub avatar: String,
}

#[tokio::main]
async fn main() {
	let config = std::fs::read_to_string("config.json").unwrap();
	let config: Config = serde_json::from_str(&config).unwrap();

	let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
	socket.set_broadcast(true).unwrap();
	socket.set_reuse_address(true).unwrap();
	#[cfg(linux)]
	socket.set_reuse_port(true).unwrap();
	socket.set_multicast_ttl_v4(225).unwrap();
	let address = SockAddr::from(std::net::SocketAddrV4::from_str("0.0.0.0:50765").unwrap());
	socket.bind(&address).unwrap();
	socket
		.join_multicast_v4(&Ipv4Addr::new(225, 0, 0, 1), &Ipv4Addr::new(0, 0, 0, 0))
		.unwrap();

	let game_socket = Arc::new(socket);
	let peer_sockets: PeerSockets = Arc::new(Mutex::new(BTreeMap::new()));

	tokio::spawn(partner_server(game_socket.clone(), "0.0.0.0:9090"));
	tokio::spawn(partner_server(game_socket.clone(), "[::1]:9090"));
	tokio::spawn(peer_send(game_socket.clone(), peer_sockets.clone()));
	game_server_connect(peer_sockets.clone(), config).await;
}

async fn partner_server_listen(stream: TcpStream, game_socket: Arc<Socket>) {
	if let Ok(mut ws_stream) = tokio_tungstenite::accept_async(stream).await {
		let mut last_frame = 0;
		while let Some(Ok(Message::Binary(data))) = ws_stream.next().await {
			let buf = &data[4..(data.len() - 4)];
			if let Ok(decoded) = <wm::Message as prost::Message>::decode(buf) {
				if let Some(heartbeat) = decoded.heart_beat {
					if let Some(frame) = heartbeat.frame_number {
						if frame <= last_frame {
							continue;
						}
						last_frame = frame;
					}
				}
			}
			game_socket.send(&data).unwrap();
		}
	}
}

async fn partner_server(game_socket: Arc<Socket>, addr: &str) {
	if let Ok(server_socket) = TcpListener::bind(addr).await {
		while let Ok((stream, _)) = server_socket.accept().await {
			tokio::spawn(partner_server_listen(stream, Arc::clone(&game_socket)));
		}
	}
}

async fn peer_send(game_socket: Arc<Socket>, peers: PeerSockets) {
	loop {
		let mut buf = std::mem::MaybeUninit::uninit_array::<1024>();
		let size = game_socket.recv(&mut buf).unwrap();
		let buf = buf
			.iter()
			.enumerate()
			.filter(|(i, _)| i < &size)
			.map(|(_, byte)| unsafe { byte.assume_init() })
			.collect::<Vec<_>>();
		if size < 2 || buf[1] == 4 {
			continue;
		}
		let mut peers = peers.lock().await;
		let futures = peers
			.iter_mut()
			.map(|(_, peer)| peer.send(Message::Binary(buf.clone())));
		futures_util::future::try_join_all(futures).await.unwrap();
	}
}

async fn game_server_connect(peers: PeerSockets, config: Config) {
	let (mut ws, _) = tokio_tungstenite::connect_async("wss://maxitune.net/clientws")
		.await
		.unwrap();
	ws.send(Message::Text(config.jwt)).await.unwrap();
	while let Some(Ok(msg)) = ws.next().await {
		if let Message::Text(msg) = msg {
			let group: Group = match serde_json::from_str(&msg) {
				Ok(group) => group,
				Err(_) => continue,
			};

			let mut peers = peers.lock().await;
			for (user, ip) in group.users.iter() {
				if !peers.contains_key(&user.id) {
					let conn_string = if ip.inner.is_ipv6() {
						format!("ws://[{}]:9090", ip.inner)
					} else {
						format!("ws://{}:9090", ip.inner)
					};
					if let Ok((peer, _)) = dbg!(tokio_tungstenite::connect_async(conn_string).await)
					{
						println!("User {} connected", user.username);
						peers.insert(user.id, peer);
					}
				}
			}

			let dead_peers = peers
				.iter()
				.filter(|(id, _)| {
					group
						.users
						.iter()
						.filter(|(guser, _)| &&guser.id == id)
						.collect::<Vec<_>>()
						.len() == 0
				})
				.map(|(id, _)| *id)
				.collect::<Vec<_>>();

			for id in dead_peers {
				println!("User {id} disconnected");
				peers.remove(&id);
			}
		} else if let Message::Ping(msg) = msg {
			_ = ws.send(Message::Pong(msg)).await;
		}
	}
}
