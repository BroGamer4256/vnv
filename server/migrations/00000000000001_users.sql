-- Add migration script here
CREATE TABLE users (
	id bigint primary key unique not null,
	username text not null,
	avatar text not null
);
