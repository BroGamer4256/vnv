#![feature(maybe_uninit_uninit_array)]
use futures_util::{SinkExt, StreamExt};
use socket2::*;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::*;

const CHECKSUM: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_MPEG_2);
type PeerSockets = Arc<Mutex<Vec<WebSocketStream<MaybeTlsStream<TcpStream>>>>>;

mod wm;

#[tokio::main]
async fn main() {
	let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
	socket.set_broadcast(true).unwrap();
	socket.set_reuse_address(true).unwrap();
	socket.set_reuse_port(true).unwrap();
	socket.set_multicast_ttl_v4(225).unwrap();
	let address = SockAddr::from(std::net::SocketAddrV4::from_str("0.0.0.0:50765").unwrap());
	socket.bind(&address).unwrap();
	socket
		.join_multicast_v4(&Ipv4Addr::new(225, 0, 0, 1), &Ipv4Addr::new(0, 0, 0, 0))
		.unwrap();

	let game_socket = Arc::new(socket);
	let peer_sockets: PeerSockets = Arc::new(Mutex::new(Vec::new()));

	tokio::spawn(partner_server(Arc::clone(&game_socket)));
	tokio::spawn(peer_send(Arc::clone(&game_socket), peer_sockets));
}

async fn partner_server_listen(stream: TcpStream, game_socket: Arc<Socket>) {
	let mut ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
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

async fn partner_server(game_socket: Arc<Socket>) {
	let server_socket = TcpListener::bind("127.0.0.1:9090")
		.await
		.expect("Failed to bind to port 9090");

	while let Ok((stream, _)) = server_socket.accept().await {
		tokio::spawn(partner_server_listen(stream, Arc::clone(&game_socket)));
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
			.map(|peer| peer.send(Message::Binary(buf.clone())));
		futures_util::future::try_join_all(futures).await.unwrap();
	}
}
