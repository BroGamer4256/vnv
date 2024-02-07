use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::*;
use axum::response::Response;
use axum::routing::*;
use axum::Router;
use jsonwebtoken::*;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::env;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

mod models;
mod web;
use models::*;
use web::*;

#[derive(Clone)]
pub struct Config {
	pub decoding_key: DecodingKey,
	pub encoding_key: EncodingKey,
	pub discord_id: String,
	pub discord_secret: String,
}

// TODO: Extract into common module
#[derive(Clone, serde::Serialize)]
pub struct Group {
	users: Vec<(User, IpAddr)>,
}

#[derive(Clone)]
pub struct AppState {
	pub config: Config,
	pub db: sqlx::Pool<sqlx::Postgres>,
	pub groups: Arc<RwLock<BTreeMap<i64, Group>>>,
	pub users: Arc<RwLock<BTreeMap<i64, IpAddr>>>,
}

#[tokio::main]
async fn main() {
	dotenvy::dotenv().expect(".env must exist");

	let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must exist");
	assert!(!database_url.is_empty(), "DATABASE_URL must not be empty");

	let db = PgPoolOptions::new()
		.max_connections(50)
		.connect(&database_url)
		.await
		.expect("Could not connect to database");
	sqlx::migrate!().run(&db).await.unwrap();

	let secret_key = env::var("SECRET_KEY").expect("SECRET_KEY must exist");
	let decoding_key = DecodingKey::from_secret(secret_key.as_bytes());
	let encoding_key = EncodingKey::from_secret(secret_key.as_bytes());
	let discord_id = env::var("DISCORD_ID").expect("DISCORD_ID must exist");
	let discord_secret = env::var("DISCORD_SECRET").expect("DISCORD_SECRET must exist");

	let config = Config {
		decoding_key,
		encoding_key,
		discord_id,
		discord_secret,
	};

	let app = Router::new()
		.route("/", get(root))
		.route("/guide", get(guide))
		.route("/create_group", get(create_group))
		.route("/join_group/:group_id", get(join_group))
		.route("/leave_group/:group_id", get(leave_group))
		.route("/config.json", get(config_download))
		.route("/login", get(login))
		.route("/clientws", get(client))
		.with_state(AppState {
			config,
			db,
			groups: Arc::new(RwLock::new(BTreeMap::new())),
			users: Arc::new(RwLock::new(BTreeMap::new())),
		});
	let listener = tokio::net::TcpListener::bind("0.0.0.0:9091").await.unwrap();
	axum::serve(listener, app).await.unwrap();
}

async fn client(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
	ws.on_upgrade(|socket| client_socket(socket, state))
}

async fn client_socket(mut socket: WebSocket, state: AppState) {
	let config = if let Some(Ok(Message::Text(config))) = socket.recv().await {
		serde_json::from_str(&config)
	} else {
		return;
	};
	if config.is_err() {
		return;
	}
	let config: UserConfig = config.unwrap();
	let ip = if let Some(ip) = config.ip {
		ip
	} else {
		return;
	};
	let user = if let Ok(user) = User::parse(&config.jwt, &state).await {
		user
	} else {
		return;
	};
	println!("User {} connected with ip {}", user.username, ip);
	{
		let mut users = state.users.write().await;
		users.insert(user.id, ip);
	}
	while socket.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
		{
			let groups = state.groups.read().await;
			let group = groups
				.iter()
				.filter(|(_, group)| group.contains_user(&user))
				.collect::<Vec<_>>();

			if let Some((_, group)) = group.first() {
				let mut group = group.to_owned().clone();
				group.users.retain(|(u, _)| u.id != user.id);
				if let Ok(json) = serde_json::to_string(&group) {
					_ = socket.send(Message::Text(json)).await;
				}
			} else {
				if let Ok(json) = serde_json::to_string(&Group { users: vec![] }) {
					_ = socket.send(Message::Text(json)).await;
				}
			}
		}

		tokio::time::sleep(std::time::Duration::from_millis(500)).await;
	}
	{
		let mut users = state.users.write().await;
		users.remove(&user.id);
	}
	{
		let mut groups = state.groups.write().await;
		let user_groups = groups
			.iter_mut()
			.filter(|(_, group)| group.contains_user(&user))
			.collect::<Vec<_>>();
		for (_, group) in user_groups {
			let index = group
				.users
				.iter()
				.position(|(u, _)| u.id == user.id)
				.unwrap();
			group.users.remove(index);
		}
		groups.retain(|_, group| group.users.len() > 0);
	}
}
