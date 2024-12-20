use axum::{
    extract::Query,
    http::{
        header::{self, HeaderMap},
        StatusCode,
    },
    routing::get,
    Router,
};
use serde::Deserialize;
use std::ops::BitXor;

#[derive(Deserialize)]
struct Addresses {
    from: String,
    key: String,
}

#[derive(Deserialize)]
struct Addresses2 {
    from: String,
    to: String,
}

async fn hello_world() -> &'static str {
    "Hello, bird!"
}

async fn seek() -> (StatusCode, HeaderMap) {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        "https://www.youtube.com/watch?v=9Gc4QTqslN4"
            .parse()
            .unwrap(),
    );
    (StatusCode::FOUND, headers)
}

async fn calc_dest_address(addresses: Query<Addresses>) -> String {
    // split addresses by "." and convert to u8
    let from_parts = addresses
        .from
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    let key_parts = addresses
        .key
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    // wrapping_add every part of the from_parts and convert to string and concatenate with "."
    let dest_address = from_parts
        .iter()
        .zip(key_parts.iter())
        .map(|(from, key)| from.wrapping_add(*key).to_string())
        .collect::<Vec<String>>()
        .join(".");
    dest_address
}

fn parse_ipv6_address(address: &str) -> Vec<u16> {
    let mut parts = address.to_string();
    let count = parts.matches(':').count();
    if count < 7 {
        let tmp = format!(":{}", ":".repeat(8 - count));
        parts = parts.replace("::", &tmp);
    }

    let parts = parts
        .split(":")
        .map(|s| if s.is_empty() { "0" } else { s })
        .map(|s| u16::from_str_radix(&s, 16).unwrap())
        .collect::<Vec<u16>>();
    parts
}

async fn calc_ipv6_dest_address(addresses: Query<Addresses>) -> String {
    let from_parts = parse_ipv6_address(&addresses.from);
    let key_parts = parse_ipv6_address(&addresses.key);

    // wrapping_add every part of the from_parts and convert to string and concatenate with ":"
    let mut dest_address = from_parts
        .iter()
        .zip(key_parts.iter())
        .map(|(from, key)| {
            let result = from.bitxor(*key);
            if result == 0 {
                String::new()
            } else {
                format!("{:x}", result)
            }
        })
        .collect::<Vec<String>>()
        .join(":");

    while dest_address.contains(":::") {
        dest_address = dest_address.replace(":::", "::");
    }
    dest_address
}

async fn calc_key_address(addresses: Query<Addresses2>) -> String {
    // split addresses by "." and convert to u8
    let from_parts = addresses
        .from
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    let to_parts = addresses
        .to
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    // wrapping_sub every part of the to_parts and convert to string and concatenate with "."
    let key_address = to_parts
        .iter()
        .zip(from_parts.iter())
        .map(|(to, from)| to.wrapping_sub(*from).to_string())
        .collect::<Vec<String>>()
        .join(".");
    key_address
}

async fn calc_ipv6_key_address(addresses: Query<Addresses2>) -> String {
    let from_parts = parse_ipv6_address(&addresses.from);
    let to_parts = parse_ipv6_address(&addresses.to);
    let mut key_address = to_parts
        .iter()
        .zip(from_parts.iter())
        // xorの逆演算 1 ^ 1 = 0, 0 ^ 1 = 1, 1 ^ 0 = 1, 0 ^ 0 = 0
        .map(|(from, key)| {
            let result = from.bitxor(*key);
            if result == 0 {
                String::new()
            } else {
                format!("{:x}", result)
            }
        })
        .collect::<Vec<String>>()
        .join(":");
    while key_address.contains(":::") {
        key_address = key_address.replace(":::", "::");
    }
    key_address
}

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let router = Router::new()
        .route("/", get(hello_world))
        .route("/-1/seek", get(seek))
        .route("/2/dest", get(calc_dest_address))
        .route("/2/key", get(calc_key_address))
        .route("/2/v6/dest", get(calc_ipv6_dest_address))
        .route("/2/v6/key", get(calc_ipv6_key_address));
    Ok(router.into())
}
