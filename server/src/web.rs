use crate::*;
use askama::Template;
use axum::{http::StatusCode, response::Redirect};
use axum_extra::extract::CookieJar;

#[derive(Template)]
#[template(path = "root.html")]
pub struct RootTemplate {
	user: Option<User>,
	groups: BTreeMap<i64, Group>,
	users: BTreeMap<i64, IpAddr>,
	in_group: bool,
}

pub async fn root(user: Option<User>, State(state): State<AppState>) -> RootTemplate {
	let groups = state.groups.read().await.clone();
	let in_group = if let Some(user) = &user {
		groups
			.iter()
			.filter(|(_, group)| group.contains_user(user))
			.collect::<Vec<_>>()
			.len() > 0
	} else {
		false
	};
	RootTemplate {
		user,
		groups,
		users: state.users.read().await.clone(),
		in_group,
	}
}

#[derive(Template)]
#[template(path = "guide.html")]
pub struct GuideTemplate {
	user: Option<User>,
	users: BTreeMap<i64, IpAddr>,
	in_group: bool,
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
		for (_, group) in groups
			.iter_mut()
			.filter(|(_, group)| group.contains_user(&user))
			.collect::<Vec<_>>()
		{
			if let Some(index) = group.users.iter().position(|(u, _)| u.id == user.id) {
				group.users.remove(index);
			}
		}
		if let Some(group) = groups.get_mut(&group_id) {
			group.users.push((user.clone(), user_ip.clone()));
		}
		groups.retain(|_, group| group.users.len() > 0);
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

pub async fn guide(user: Option<User>, State(state): State<AppState>) -> GuideTemplate {
	let groups = state.groups.read().await.clone();
	let in_group = if let Some(user) = &user {
		groups
			.iter()
			.filter(|(_, group)| group.contains_user(user))
			.collect::<Vec<_>>()
			.len() > 0
	} else {
		false
	};
	GuideTemplate {
		user,
		users: state.users.read().await.clone(),
		in_group,
	}
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
