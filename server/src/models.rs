use std::collections::HashMap;

use crate::*;
use axum::http::header::*;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::Redirect;
use axum::RequestPartsExt;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};

impl Group {
	pub fn contains_user(&self, user: &User) -> bool {
		self.users
			.iter()
			.filter(|(u, _)| u.id == user.id)
			.collect::<Vec<_>>()
			.len() > 0
	}
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
	pub id: i64,
	pub username: String,
	pub avatar: String,
}

#[derive(Serialize, Deserialize)]
pub struct Token {
	pub exp: i64,
	pub user_id: i64,
}

impl User {
	pub async fn parse(token: &str, state: &AppState) -> Result<Self, StatusCode> {
		let token_data = jsonwebtoken::decode::<Token>(
			&token,
			&state.config.decoding_key,
			&Validation::default(),
		)
		.map_err(|_| StatusCode::UNAUTHORIZED)?;
		let user = sqlx::query!(
			"SELECT * FROM users WHERE id = $1",
			token_data.claims.user_id
		)
		.fetch_one(&state.db)
		.await
		.map_err(|_| StatusCode::UNAUTHORIZED)?;

		Ok(User {
			id: user.id,
			username: user.username,
			avatar: user.avatar,
		})
	}
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for User
where
	S: Send + Sync,
	AppState: FromRef<S>,
{
	type Rejection = StatusCode;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
		let cookies = parts
			.extract::<CookieJar>()
			.await
			.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
		let cookie = cookies.get(&AUTHORIZATION.to_string());
		let token = match cookie {
			Some(cookie) => String::from(cookie.value()),
			None => {
				let auth = parts
					.headers
					.get(AUTHORIZATION)
					.ok_or(StatusCode::UNAUTHORIZED)?;
				auth.to_str()
					.map_err(|_| StatusCode::BAD_REQUEST)?
					.replace("Bearer ", "")
			}
		};
		let state = AppState::from_ref(state);
		Self::parse(&token, &state).await
	}
}

// https://discord.com/api/oauth2/authorize?client_id=1202965157790093393&response_type=code&redirect_uri=https%3A%2F%2Fmaxitune.net%2Flogin&scope=identify
pub async fn login(
	State(state): State<AppState>,
	Query(params): Query<HashMap<String, String>>,
	cookies: CookieJar,
) -> Result<(CookieJar, Redirect), StatusCode> {
	let code = params.get("code").ok_or(StatusCode::UNAUTHORIZED)?;

	let mut params: HashMap<&str, &str> = std::collections::HashMap::new();
	params.insert("grant_type", "authorization_code");
	params.insert("redirect_uri", "https://maxitune.net/login");
	params.insert("code", &code);

	#[derive(Serialize, Deserialize)]
	struct DiscordTokenResponse {
		access_token: String,
		token_type: String,
		expires_in: i64,
		refresh_token: String,
		scope: String,
	}

	#[derive(Serialize, Deserialize)]
	struct DiscordUser {
		id: String,
		global_name: String,
		discriminator: String,
		avatar: Option<String>,
	}

	let response = reqwest::Client::new()
		.post("https://discord.com/api/v10/oauth2/token")
		.basic_auth(state.config.discord_id, Some(state.config.discord_secret))
		.form(&params)
		.send()
		.await
		.map_err(|_| StatusCode::BAD_REQUEST)?;
	if !response.status().is_success() {
		return Err(StatusCode::BAD_REQUEST);
	};

	let response: DiscordTokenResponse = response
		.json()
		.await
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	let response = reqwest::Client::new()
		.get("https://discord.com/api/users/@me")
		.header(
			"authorization",
			format!("{} {}", response.token_type, response.access_token),
		)
		.send()
		.await
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	if !response.status().is_success() {
		return Err(StatusCode::INTERNAL_SERVER_ERROR);
	}

	let response: DiscordUser = response
		.json()
		.await
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
	let id: i64 = response
		.id
		.parse()
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
	let avatar = if let Some(avatar) = response.avatar {
		format!("https://cdn.discordapp.com/avatars/{}/{}.png", id, avatar)
	} else {
		let discriminator: i32 = response.discriminator.parse().unwrap_or_default();
		format!(
			"https://cdn.discordapp.com/embed/avatars/{}.png",
			discriminator % 5
		)
	};
	sqlx::query!(
		"INSERT INTO users VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET avatar = excluded.avatar",
		id,
		response.global_name,
		avatar
	)
	.execute(&state.db)
	.await
	.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	let time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap()
		.as_secs() as i64;
	let token = Token {
		exp: time + 60 * 24 * 60 * 60,
		user_id: id,
	};

	if let Ok(encoded) = encode(&Header::default(), &token, &state.config.encoding_key) {
		let mut cookie = Cookie::new(AUTHORIZATION.to_string(), encoded);
		cookie.set_same_site(axum_extra::extract::cookie::SameSite::Lax);
		return Ok((cookies.add(cookie), Redirect::to("/")));
	}

	Err(StatusCode::UNAUTHORIZED)
}

#[derive(Deserialize, Serialize)]
pub struct UserConfig {
	pub jwt: String,
	pub ip: Option<IpAddr>,
}
