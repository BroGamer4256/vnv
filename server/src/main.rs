use axum::extract::ws::WebSocket;
use axum::extract::WebSocketUpgrade;
use axum::response::Response;
use axum::routing::*;
use axum::Router;
use jsonwebtoken::*;
use sqlx::postgres::PgPoolOptions;
use std::env;

mod user;
use user::*;

#[derive(Clone)]
pub struct Config {
	pub decoding_key: DecodingKey,
	pub encoding_key: EncodingKey,
	pub discord_id: String,
	pub discord_secret: String,
}

#[derive(Clone)]
pub struct AppState {
	pub config: Config,
	pub db: sqlx::Pool<sqlx::Postgres>,
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
		.route("/login", get(login))
		.route("/clientws", get(client))
		.with_state(AppState { config, db });
	let listener = tokio::net::TcpListener::bind("0.0.0.0:9091").await.unwrap();
	axum::serve(listener, app).await.unwrap();
}

async fn root(user: User) {}

async fn client(ws: WebSocketUpgrade, user: User) -> Response {
	ws.on_upgrade(|socket| client_socket(socket, user))
}

async fn client_socket(mut socket: WebSocket, user: User) {
	socket.close().await.unwrap();
}
