//! `provider_config`: the persisted, UI-rendered settings (spec §Info
//! `config_schema`, invariant I2 — no secrets in configuration).

use serde_json::{json, Value};

pub const DEFAULT_IMAGE: &str =
    "ghcr.io/abipalli/buzz-sprig@sha256:9e8e8134868e76481688bd459d80fdabb582bd51260e5ac2c0079323b72ceca4";
pub const DEFAULT_INACTIVITY_SECONDS: u64 = 7200;
pub const DEFAULT_NETWORK: &str = "buzz-agents";
pub const DEFAULT_CPUS: &str = "2";
pub const DEFAULT_MEMORY: &str = "4g";

const MAX_FIELDS: usize = 20;
const MAX_BYTES: usize = 64 * 1024;
const SECRET_WORDS: [&str; 5] = ["secret", "password", "token", "key", "credential"];

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderConfig {
    pub host: Option<String>,
    pub context: Option<String>,
    pub image: String,
    pub network: Option<String>,
    pub add_hosts: Vec<String>,
    pub ca_volume: Option<String>,
    pub ca_volume_subpath: Option<String>,
    pub inactivity_seconds: u64,
    pub cpus: String,
    pub memory: String,
}

fn validate_shape(cfg: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    let map = match cfg {
        Value::Null => return Ok(empty_map()),
        Value::Object(map) => map,
        _ => return Err("provider_config must be an object".into()),
    };
    if map.len() > MAX_FIELDS {
        return Err(format!(
            "provider_config has {} fields; the limit is {MAX_FIELDS}",
            map.len()
        ));
    }
    if cfg.to_string().len() > MAX_BYTES {
        return Err(format!("provider_config exceeds {MAX_BYTES} bytes"));
    }
    for (key, value) in map {
        if matches!(value, Value::Object(_) | Value::Array(_)) {
            return Err(format!("provider_config.{key} must be a scalar"));
        }
        let lower = key.to_ascii_lowercase();
        if lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| SECRET_WORDS.contains(&word))
        {
            return Err(format!(
                "provider_config.{key} looks like a secret field; secrets must never be stored in provider settings"
            ));
        }
    }
    Ok(map)
}

fn empty_map() -> &'static serde_json::Map<String, Value> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(serde_json::Map::new)
}

fn string_field(
    map: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match map.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(_) => Err(format!("provider_config.{field} must be a string")),
    }
}

/// A Docker daemon address as `docker -H` accepts it. The URL may name a user
/// but never carry a password: credentials stay in the SSH agent or config.
pub fn validate_host(host: &str) -> Result<(), String> {
    let Some((scheme, rest)) = host.split_once("://") else {
        return Err(format!(
            "Docker host {host:?} must look like ssh://user@server or tcp://server:2376"
        ));
    };
    if !matches!(scheme, "ssh" | "tcp" | "unix" | "npipe") || rest.is_empty() {
        return Err(format!(
            "Docker host {host:?} must start with ssh://, tcp://, unix:// or npipe://"
        ));
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("Docker host {host:?} contains whitespace"));
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority
        .split_once('@')
        .is_some_and(|(user, _)| user.contains(':'))
    {
        return Err(
            "Docker host must not contain a password; use an SSH key or your SSH config".into(),
        );
    }
    Ok(())
}

fn is_docker_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

fn is_host_entry(s: &str) -> bool {
    let Some((host, target)) = s.split_once(':') else {
        return false;
    };
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
        && (target == "host-gateway" || target.parse::<std::net::IpAddr>().is_ok())
}

pub fn parse(cfg: &Value) -> Result<ProviderConfig, String> {
    let map = validate_shape(cfg)?;

    let host = string_field(map, "host")?;
    if let Some(h) = &host {
        validate_host(h)?;
    }
    let context = string_field(map, "context")?;
    let context = if host.is_some() { None } else { context };
    if let Some(c) = &context {
        if !is_docker_name(c) {
            return Err(format!(
                "provider_config.context {c:?} is not a valid Docker context name"
            ));
        }
    }

    let image = string_field(map, "image")?.unwrap_or_else(|| DEFAULT_IMAGE.to_string());
    if image.ends_with(":latest") || !image.contains(['@', ':']) {
        return Err(format!(
            "provider_config.image {image:?} must be pinned by tag or digest (\":latest\" and untagged references are refused)"
        ));
    }

    let network =
        Some(string_field(map, "network")?.unwrap_or_else(|| DEFAULT_NETWORK.to_string()));
    if let Some(n) = &network {
        if !is_docker_name(n) || matches!(n.as_str(), "host" | "none") {
            return Err(format!(
                "provider_config.network {n:?} is not an allowed Docker network"
            ));
        }
    }

    let add_hosts: Vec<String> = string_field(map, "add_hosts")?
        .map(|s| {
            s.split(',')
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(bad) = add_hosts.iter().find(|h| !is_host_entry(h)) {
        return Err(format!(
            "provider_config.add_hosts entry {bad:?} must be host:ip or host:host-gateway"
        ));
    }

    let ca_volume = string_field(map, "ca_volume")?;
    let ca_volume_subpath = string_field(map, "ca_volume_subpath")?;
    match (&ca_volume, &ca_volume_subpath) {
        (Some(v), sub) => {
            if !is_docker_name(v) {
                return Err(format!(
                    "provider_config.ca_volume {v:?} is not a valid volume name"
                ));
            }
            let Some(p) = sub else {
                return Err("provider_config.ca_volume needs ca_volume_subpath: only the certificate file is mounted, never the whole volume".into());
            };
            if p.starts_with('/') || p.split('/').any(|seg| seg == ".." || seg.is_empty()) {
                return Err(format!("provider_config.ca_volume_subpath {p:?} must be a relative path inside the volume"));
            }
        }
        (None, Some(_)) => {
            return Err("provider_config.ca_volume_subpath requires ca_volume".into())
        }
        (None, None) => {}
    }

    let inactivity_seconds = match map.get("inactivity_seconds") {
        None | Some(Value::Null) => DEFAULT_INACTIVITY_SECONDS,
        Some(Value::Number(n)) => n
            .as_u64()
            .ok_or("provider_config.inactivity_seconds must be a non-negative integer")?,
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map_err(|_| "provider_config.inactivity_seconds must be a non-negative integer")?,
        Some(_) => {
            return Err("provider_config.inactivity_seconds must be a non-negative integer".into())
        }
    };

    let cpus = string_field(map, "cpus")?.unwrap_or_else(|| DEFAULT_CPUS.to_string());
    if !cpus.parse::<f64>().is_ok_and(|c| c > 0.0) {
        return Err(format!(
            "provider_config.cpus {cpus:?} must be a positive number"
        ));
    }
    let memory = string_field(map, "memory")?.unwrap_or_else(|| DEFAULT_MEMORY.to_string());
    let (digits, unit) = memory.split_at(
        memory
            .trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .len(),
    );
    if digits.is_empty()
        || !digits.chars().all(|c| c.is_ascii_digit())
        || !matches!(
            unit.to_ascii_lowercase().as_str(),
            "" | "b" | "k" | "m" | "g"
        )
    {
        return Err(format!(
            "provider_config.memory {memory:?} must look like 512m or 4g"
        ));
    }

    Ok(ProviderConfig {
        host,
        context,
        image,
        network,
        add_hosts,
        ca_volume,
        ca_volume_subpath,
        inactivity_seconds,
        cpus,
        memory,
    })
}

pub fn config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "host": {"type": "string", "title": "Docker host", "description": "The server that runs the agent, e.g. ssh://you@your-server (port: ssh://you@your-server:2222). Uses your SSH keys; empty uses this machine's Docker.", "default": ""},
            "context": {"type": "string", "title": "Docker context", "description": "Alternative to Docker host: a name from `docker context ls`. Ignored when Docker host is set.", "default": ""},
            "image": {"type": "string", "title": "Agent image", "description": "Must contain buzz-acp (buzz-sprig or an image built FROM it). Pinned by tag or digest.", "default": DEFAULT_IMAGE},
            "network": {"type": "string", "title": "Docker network", "description": "Network the agent joins; created if missing. Its DNS follows the server's, so names your server resolves work for agents too.", "default": DEFAULT_NETWORK},
            "add_hosts": {"type": "string", "title": "Extra hosts", "description": "Comma-separated host:ip or host:host-gateway entries, e.g. relay.example.internal:host-gateway", "default": ""},
            "ca_volume": {"type": "string", "title": "CA volume", "description": "Volume on the Docker host holding a private root CA the agent must trust (for self-hosted relays).", "default": ""},
            "ca_volume_subpath": {"type": "string", "title": "CA file in volume", "description": "Path of the CA certificate inside ca_volume, e.g. certs/root_ca.crt", "default": ""},
            "inactivity_seconds": {"type": "integer", "title": "Stop after inactivity (seconds)", "description": "The agent stops itself after this long without work. 0 keeps it running until you stop it.", "default": DEFAULT_INACTIVITY_SECONDS, "minimum": 0},
            "cpus": {"type": "string", "title": "CPU limit", "default": DEFAULT_CPUS},
            "memory": {"type": "string", "title": "Memory limit", "default": DEFAULT_MEMORY}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_hosts_are_validated() {
        for ok in [
            "ssh://abdev@100.64.0.3:2283",
            "ssh://server",
            "tcp://10.0.0.2:2376",
            "unix:///var/run/docker.sock",
        ] {
            assert!(parse(&json!({"host": ok})).is_ok(), "{ok} refused");
        }
        for bad in [
            "server",
            "http://server",
            "ssh://",
            "ssh://me:hunter2@server",
            "ssh://me@server bad",
        ] {
            assert!(parse(&json!({"host": bad})).is_err(), "{bad} accepted");
        }
        let both = parse(&json!({"host": "ssh://s", "context": "c"})).unwrap();
        assert_eq!(
            (both.host.as_deref(), both.context),
            (Some("ssh://s"), None),
            "host must win over context"
        );
    }

    #[test]
    fn empty_config_takes_defaults() {
        let c = parse(&json!({})).unwrap();
        assert_eq!(c.image, DEFAULT_IMAGE);
        assert_eq!(c.network.as_deref(), Some(DEFAULT_NETWORK));
        assert_eq!(c.inactivity_seconds, DEFAULT_INACTIVITY_SECONDS);
        assert_eq!(
            (c.cpus.as_str(), c.memory.as_str()),
            (DEFAULT_CPUS, DEFAULT_MEMORY)
        );
        assert_eq!(parse(&Value::Null).unwrap(), c);
    }

    #[test]
    fn schema_defaults_parse_cleanly() {
        let schema = config_schema();
        let defaults: serde_json::Map<String, Value> = schema["properties"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v["default"].clone()))
            .collect();
        assert_eq!(
            parse(&Value::Object(defaults)).unwrap(),
            parse(&json!({})).unwrap()
        );
    }

    #[test]
    fn secret_shaped_keys_are_refused() {
        for key in [
            "api_key",
            "password",
            "registry_token",
            "ssh-key-path",
            "Credential",
        ] {
            assert!(parse(&json!({key: "x"})).is_err(), "{key} accepted");
        }
        assert!(
            parse(&json!({"keyring_hint": "x"})).is_ok(),
            "word match, not substring"
        );
    }

    #[test]
    fn nested_and_oversized_configs_are_refused() {
        assert!(parse(&json!({"network": {"name": "x"}})).is_err());
        assert!(parse(&json!({"add_hosts": ["a:1.2.3.4"]})).is_err());
        let many: serde_json::Map<String, Value> =
            (0..21).map(|i| (format!("f{i}"), json!("x"))).collect();
        assert!(parse(&Value::Object(many)).is_err());
        assert!(parse(&json!("not an object")).is_err());
    }

    #[test]
    fn unpinned_images_are_refused() {
        for bad in ["ghcr.io/block/buzz-sprig:latest", "buzz-sprig"] {
            assert!(parse(&json!({"image": bad})).is_err(), "{bad} accepted");
        }
        for ok in ["buzz-sprig-nativeroots:da651ad", "ghcr.io/x/y@sha256:abc"] {
            assert!(parse(&json!({"image": ok})).is_ok(), "{ok} refused");
        }
    }

    #[test]
    fn host_network_and_bad_hosts_are_refused() {
        assert!(parse(&json!({"network": "host"})).is_err());
        assert!(parse(&json!({"add_hosts": "relay.internal"})).is_err());
        assert!(parse(&json!({"add_hosts": "relay.internal:not-an-ip"})).is_err());
        let c =
            parse(&json!({"add_hosts": "a.internal:host-gateway, b.internal:10.0.0.3"})).unwrap();
        assert_eq!(
            c.add_hosts,
            ["a.internal:host-gateway", "b.internal:10.0.0.3"]
        );
    }

    #[test]
    fn ca_subpath_must_stay_inside_the_volume() {
        assert!(parse(&json!({"ca_volume_subpath": "certs/root_ca.crt"})).is_err());
        for bad in ["/etc/passwd", "../x", "certs//x"] {
            assert!(
                parse(&json!({"ca_volume": "v", "ca_volume_subpath": bad})).is_err(),
                "{bad} accepted"
            );
        }
        assert!(
            parse(&json!({"ca_volume": "docker_stepca_data"})).is_err(),
            "whole-volume mount allowed"
        );
        assert!(parse(
            &json!({"ca_volume": "docker_stepca_data", "ca_volume_subpath": "certs/root_ca.crt"})
        )
        .is_ok());
    }

    #[test]
    fn zero_inactivity_is_a_legal_choice() {
        assert_eq!(
            parse(&json!({"inactivity_seconds": 0}))
                .unwrap()
                .inactivity_seconds,
            0
        );
        assert_eq!(
            parse(&json!({"inactivity_seconds": "60"}))
                .unwrap()
                .inactivity_seconds,
            60
        );
        assert!(parse(&json!({"inactivity_seconds": -1})).is_err());
    }

    #[test]
    fn resource_limits_are_validated() {
        assert!(parse(&json!({"cpus": "0"})).is_err());
        assert!(parse(&json!({"memory": "lots"})).is_err());
        assert!(parse(&json!({"cpus": 1.5, "memory": "512m"})).is_ok());
    }
}
