use axum::{
    body::Bytes,
    extract::Query,
    http::{
        header::{self, HeaderMap, CONTENT_TYPE},
        StatusCode,
    },
    routing::{get, post},
    Router,
};
use cargo_manifest::{Manifest, MaybeInherited};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::ops::BitXor;
use toml;

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

async fn parse_manifest(headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    let content_type_header = headers.get(CONTENT_TYPE);
    let content_type = match content_type_header {
        Some(content_type_header) => content_type_header,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new()),
    };

    let toml_str = if content_type == "application/json" {
        // JSONをTOMLに変換
        let json_value: JsonValue = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON".to_string()),
        };
        match toml::to_string_pretty(&json_value) {
            Ok(s) => s,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "Failed to convert JSON to TOML".to_string(),
                )
            }
        }
    } else if content_type == "application/yaml" {
        // YAMLをTOMLに変換
        let yaml_value: YamlValue = match serde_yaml::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid YAML".to_string()),
        };
        match toml::to_string_pretty(&yaml_value) {
            Ok(s) => s,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "Failed to convert YAML to TOML".to_string(),
                )
            }
        }
    } else if content_type == "application/toml" {
        match String::from_utf8(body.to_vec()) {
            Ok(s) => s,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid TOML".to_string()),
        }
    } else {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new());
    };

    let manifest = match Manifest::from_slice(toml_str.as_bytes()) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid manifest".to_string()),
    };

    let package = match manifest.package {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
    };

    let keywords = match package.keywords {
        Some(k) => k,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
    };

    let keywords = match keywords {
        MaybeInherited::Inherited { .. } => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
        MaybeInherited::Local(k) => k,
    };

    if !keywords.contains(&"Christmas 2024".to_string()) {
        return (
            StatusCode::BAD_REQUEST,
            "Magic keyword not provided".to_string(),
        );
    }

    let metadata = match package.metadata {
        Some(m) => m,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };

    let orders = match metadata.get("orders") {
        Some(o) => o,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };
    let orders = match orders.as_array() {
        Some(o) => o,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };

    let mut outputs = Vec::new();
    for order in orders {
        let item = match order.get("item") {
            Some(i) => i,
            None => continue,
        };
        let quantity = match order.get("quantity") {
            Some(q) => q,
            None => continue,
        };
        let item = match item.as_str() {
            Some(i) => i,
            None => continue,
        };
        let quantity = match quantity.as_integer() {
            Some(q) => q,
            None => continue,
        };
        outputs.push(format!("{}: {}", item, quantity));
    }
    if outputs.is_empty() {
        return (StatusCode::NO_CONTENT, String::new());
    }
    (StatusCode::OK, outputs.join("\n"))
}

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let router = Router::new()
        .route("/", get(hello_world))
        .route("/-1/seek", get(seek))
        .route("/2/dest", get(calc_dest_address))
        .route("/2/key", get(calc_key_address))
        .route("/2/v6/dest", get(calc_ipv6_dest_address))
        .route("/2/v6/key", get(calc_ipv6_key_address))
        .route("/5/manifest", post(parse_manifest));
    Ok(router.into())
}
