use crate::*;
use askama::Template;
use axum::{http::StatusCode, response::Redirect};
use axum_extra::extract::CookieJar;

#[derive(Template)]
#[template(path = "root.html")]
pub struct RootTemplate {
	user: Option<User>,
	groups: BTreeMap<i64, Group>,
}

pub async fn root(user: Option<User>, State(state): State<AppState>) -> RootTemplate {
	RootTemplate {
		user,
		groups: state.groups.read().await.clone(),
	}
}

#[derive(Template)]
#[template(path = "guide.html")]
pub struct GuideTemplate {
	user: Option<User>,
}

pub async fn create_group(user: User, State(state): State<AppState>) -> Redirect {
	if let Some(user_ip) = state.users.read().await.get(&user.id) {
		let group = Group {
			users: vec![(user.clone(), user_ip.clone())],
		};
		let mut groups = state.groups.write().await;
		groups.insert(user.id, group);
	}
	Redirect::to("/")
}

pub async fn join_group(
	user: User,
	State(state): State<AppState>,
	Path(group_id): Path<i64>,
) -> Redirect {
	if let Some(user_ip) = state.users.read().await.get(&user.id) {
		let mut groups = state.groups.write().await;
		if let Some(group) = groups.get_mut(&group_id) {
			group.users.push((user.clone(), user_ip.clone()));
		}
	}
	Redirect::to("/")
}

pub async fn leave_group(
	user: User,
	State(state): State<AppState>,
	Path(group_id): Path<i64>,
) -> Redirect {
	let mut groups = state.groups.write().await;
	if let Some(group) = groups.get_mut(&group_id) {
		if let Some(index) = group.users.iter().position(|(u, _)| u.id == user.id) {
			group.users.remove(index);
			groups.retain(|_, group| group.users.len() > 0);
		}
	}

	Redirect::to("/")
}

pub async fn guide(user: Option<User>) -> GuideTemplate {
	GuideTemplate { user }
}

pub async fn config_download(_: User, jar: CookieJar) -> Result<String, StatusCode> {
	let cookie = jar
		.get("authorization")
		.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
	let jwt = cookie.value();

	#[derive(serde::Serialize)]
	struct Config {
		jwt: String,
	}
	let config = Config {
		jwt: String::from(jwt),
	};

	serde_json::to_string(&config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
