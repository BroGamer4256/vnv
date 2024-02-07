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

#[derive(Deserialize, Serialize)]
struct Config {
	pub jwt: String,
	pub ip: Option<IpAddr>,
}

#[derive(Clone, Deserialize)]
pub struct Group {
	users: Vec<(User, IpAddr)>,
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
	let mut config: Config = serde_json::from_str(&config).unwrap();
	if config.ip.is_none() {
		let ip = reqwest::get("https://api.ipify.org/")
			.await
			.unwrap()
			.text()
			.await
			.unwrap();
		let ip = IpAddr::from_str(&ip).unwrap();
		config.ip = Some(ip);
	}

	let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
	socket.set_broadcast(true).unwrap();
	socket.set_reuse_address(true).unwrap();
	#[cfg(linux)]
	socket.set_reuse_port(true).unwrap();
	socket.set_multicast_ttl_v4(225).unwrap();
	let address = SockAddr::from(std::net::SocketAddrV4::from_str("0.0.0.0:50765").unwrap());
	socket
		.join_multicast_v4(&Ipv4Addr::new(225, 0, 0, 1), &Ipv4Addr::new(0, 0, 0, 0))
		.unwrap();
	socket.bind(&address).unwrap();

	let game_socket = Arc::new(socket);
	let peer_sockets: PeerSockets = Arc::new(Mutex::new(BTreeMap::new()));

	#[cfg(windows)]
	{
		let local_ip = local_ip_address::local_ip().unwrap();
		let local_ip = format!("{local_ip}:9090");
		tokio::spawn(partner_server(game_socket.clone(), local_ip));
	}
	tokio::spawn(partner_server(game_socket.clone(), ":::9090"));
	tokio::spawn(peer_send(game_socket.clone(), peer_sockets.clone()));
	game_server_connect(peer_sockets.clone(), config).await;
}

async fn partner_server_listen(stream: TcpStream, game_socket: Arc<Socket>) {
	if let Ok(mut ws_stream) = tokio_tungstenite::accept_async(stream).await {
		while let Some(Ok(Message::Binary(data))) = ws_stream.next().await {
			_ = game_socket.send_to(
				&data,
				&SockAddr::from(std::net::SocketAddrV4::from_str("225.0.0.1:50765").unwrap()),
			);
		}
	} else {
		dbg!("Failed to accept stream as websocket");
	}
}

async fn partner_server<A: tokio::net::ToSocketAddrs + std::fmt::Display>(
	game_socket: Arc<Socket>,
	addr: A,
) {
	let server_socket = TcpListener::bind(&addr).await.expect("Failed to bind");
	while let Ok((stream, _)) = server_socket.accept().await {
		tokio::spawn(partner_server_listen(stream, Arc::clone(&game_socket)));
	}
}

async fn peer_send(game_socket: Arc<Socket>, peers: PeerSockets) {
	let mut self_id = u8::MAX;
	let mut buf = std::mem::MaybeUninit::uninit_array::<32767>();
	loop {
		let size = match game_socket.recv(&mut buf) {
			Ok(size) => size,
			Err(e) => {
				dbg!(e);
				continue;
			}
		};
		let buf = buf
			.iter()
			.enumerate()
			.filter(|(i, _)| i < &size)
			.map(|(_, byte)| unsafe { byte.assume_init() })
			.collect::<Vec<_>>();
		if self_id == u8::MAX {
			if size > 2 && buf[1] != 4 {
				self_id = buf[1];
				dbg!(self_id);
			}
		}
		if size < 2 || buf[1] == 4 || buf[1] != self_id {
			continue;
		}
		let mut peers = peers.lock().await;
		for (_, peer) in peers.iter_mut() {
			_ = peer.send(Message::Binary(buf.clone())).await;
		}
	}
}

async fn game_server_connect(peers: PeerSockets, config: Config) {
	let (mut ws, _) = tokio_tungstenite::connect_async("wss://maxitune.net/clientws")
		.await
		.unwrap();
	ws.send(Message::Text(serde_json::to_string(&config).unwrap()))
		.await
		.unwrap();
	while let Some(Ok(msg)) = ws.next().await {
		if let Message::Text(msg) = msg {
			let group: Group = match serde_json::from_str(&msg) {
				Ok(group) => group,
				Err(e) => {
					dbg!(e);
					return;
				}
			};

			let mut peers = peers.lock().await;
			for (user, ip) in group.users.iter() {
				if !peers.contains_key(&user.id) {
					let conn_string = if ip.is_ipv6() {
						format!("ws://[{}]:9090", ip)
					} else {
						format!("ws://{}:9090", ip)
					};
					match tokio_tungstenite::connect_async(conn_string).await {
						Ok((peer, _)) => {
							println!("Connected to {}", user.username);
							peers.insert(user.id, peer);
						}
						Err(e) => {
							dbg!(e);
						}
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
		} else {
			dbg!(msg);
		}
	}
}
