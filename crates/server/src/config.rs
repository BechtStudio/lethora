//! The server's settings: `lethora-server.toml` (a missing file means the
//! defaults), then `LETHORA_<KEY>` environment variables, one per setting
//! (e.g. `LETHORA_LISTEN`, `LETHORA_PUBLIC_URL`, `LETHORA_MESSAGE_BYTES`).
//! Top-level keys: `listen`, `data_dir`, `public_url`, `trusted_proxies`
//! (in the environment a comma-separated list); the limits sit in a
//! `[limits]` table, in the environment without a prefix. Sizes are in
//! bytes, durations in seconds.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The server's settings (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The address to listen on.
    pub listen: SocketAddr,
    /// Where projects are stored.
    pub data_dir: PathBuf,
    /// The origin clients reach the server at (e.g. `https://sync.example`),
    /// signed in every hello and used in invite links. Required unless the
    /// server listens on a loopback address, where it defaults to
    /// `http://127.0.0.1:<port>`.
    pub public_url: Option<String>,
    /// Reverse proxies whose `X-Forwarded-For` header names the client.
    pub trusted_proxies: Vec<IpAddr>,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8740)),
            data_dir: PathBuf::from("lethora-data"),
            public_url: None,
            trusted_proxies: Vec::new(),
            limits: Limits::default(),
        }
    }
}

/// Limits (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Largest WebSocket message.
    pub message_bytes: u64,
    /// Largest asset or snapshot upload.
    pub asset_bytes: u64,
    /// Storage per project.
    pub project_bytes: u64,
    /// New projects per client IP address in 24 hours.
    pub projects_per_ip_per_day: u32,
    /// Open WebSocket connections per client IP address.
    pub connections_per_ip: u32,
    /// Messages a connection may send per second, and in a burst.
    pub messages_per_second: u32,
    pub message_burst: u32,
    /// Bytes a connection may send per second, and in a burst (at least
    /// `message_bytes`).
    pub bytes_per_second: u64,
    pub byte_burst: u64,
    /// Presence messages per second a connection may send; more are dropped.
    pub presence_per_second: u32,
    /// Largest presence message.
    pub presence_bytes: u64,
    /// Largest catch-up message (whole records; a larger record goes alone).
    pub chunk_bytes: u64,
    /// How long a client has to answer the challenge.
    pub hello_seconds: u64,
    /// A connection that sends nothing for this long is closed (clients
    /// send pings).
    pub idle_seconds: u64,
    /// A project nobody has used for this long is unloaded from memory.
    pub unload_seconds: u64,
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: u64 = 1 << 20;
        Self {
            message_bytes: 4 * MIB,
            asset_bytes: 100 * MIB,
            project_bytes: 2048 * MIB,
            projects_per_ip_per_day: 10,
            connections_per_ip: 20,
            messages_per_second: 50,
            message_burst: 200,
            bytes_per_second: 2 * MIB,
            byte_burst: 16 * MIB,
            presence_per_second: 20,
            presence_bytes: 64 * 1024,
            chunk_bytes: MIB,
            hello_seconds: 10,
            idle_seconds: 120,
            unload_seconds: 300,
        }
    }
}

/// The top-level keys that aren't limits.
const TOP_LEVEL: [&str; 4] = ["listen", "data_dir", "public_url", "trusted_proxies"];

impl Config {
    /// The settings in TOML file `path` (`None` or a missing file: the
    /// defaults), overridden by the `LETHORA_*` variables in `env`.
    pub fn load(
        path: Option<&Path>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, String> {
        let text = match path.map(std::fs::read_to_string) {
            Some(Ok(text)) => text,
            Some(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Some(Err(e)) => {
                return Err(format!("can't read {}: {e}", path.expect("some").display()));
            }
            None => String::new(),
        };
        let mut table: toml::Table = toml::from_str(&text).map_err(|e| e.to_string())?;
        // Gone (tokens now last as long as their connection); old files still load.
        if let Some(toml::Value::Table(limits)) = table.get_mut("limits") {
            limits.remove("token_seconds");
        }
        let limit_keys = toml::Table::try_from(Limits::default()).map_err(|e| e.to_string())?;
        for (name, value) in env {
            let Some(key) = name.strip_prefix("LETHORA_") else {
                continue;
            };
            let key = key.to_ascii_lowercase();
            if key == "trusted_proxies" {
                let list = value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| toml::Value::String(s.to_owned()))
                    .collect();
                table.insert(key, toml::Value::Array(list));
            } else if TOP_LEVEL.contains(&key.as_str()) {
                table.insert(key, toml::Value::String(value));
            } else if limit_keys.contains_key(&key) {
                let number = value
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| format!("{name} must be a whole number"))?;
                let limits = table
                    .entry("limits")
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                let toml::Value::Table(limits) = limits else {
                    return Err("limits must be a table".into());
                };
                limits.insert(key, toml::Value::Integer(number));
            }
        }
        let config: Config = table
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        config.check()?;
        Ok(config)
    }

    /// Checks that the limits make sense.
    fn check(&self) -> Result<(), String> {
        let l = &self.limits;
        if l.message_bytes < 1024 || l.chunk_bytes == 0 || l.chunk_bytes > l.message_bytes / 2 {
            return Err("limits: message_bytes ≥ 1024, 0 < chunk_bytes ≤ message_bytes / 2".into());
        }
        if l.byte_burst < l.message_bytes || l.messages_per_second == 0 || l.message_burst == 0 {
            return Err("limits: byte_burst ≥ message_bytes, message rates above 0".into());
        }
        if l.bytes_per_second == 0 {
            return Err("limits: bytes_per_second above 0".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Vec<(String, String)> {
        vars.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_file_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert_eq!(Config::load(Some(&missing), []).unwrap(), Config::default());
        assert_eq!(Config::load(None, []).unwrap(), Config::default());

        let path = dir.path().join("lethora-server.toml");
        // `token_seconds`: an old setting, ignored.
        std::fs::write(
            &path,
            "listen = \"0.0.0.0:9000\"\n\
             public_url = \"https://sync.example\"\n\
             trusted_proxies = [\"10.0.0.1\"]\n\
             [limits]\n\
             message_bytes = 2097152\n\
             connections_per_ip = 5\n\
             token_seconds = 600\n",
        )
        .unwrap();
        let config = Config::load(Some(&path), []).unwrap();
        assert_eq!(config.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.public_url.as_deref(), Some("https://sync.example"));
        assert_eq!(
            config.trusted_proxies,
            ["10.0.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(config.limits.message_bytes, 2 << 20);
        assert_eq!(config.limits.connections_per_ip, 5);
        assert_eq!(config.limits.asset_bytes, 100 << 20);

        let config = Config::load(
            Some(&path),
            env(&[
                ("LETHORA_LISTEN", "127.0.0.1:1"),
                ("LETHORA_DATA_DIR", "/srv/lethora"),
                ("LETHORA_TRUSTED_PROXIES", "10.0.0.2, ::1"),
                ("LETHORA_CONNECTIONS_PER_IP", "7"),
                ("LETHORA_PROJECT_BYTES", "1000000"),
                ("LETHORA_SOMETHING_ELSE", "x"),
                ("PATH", "x"),
            ]),
        )
        .unwrap();
        assert_eq!(config.listen, "127.0.0.1:1".parse().unwrap());
        assert_eq!(config.data_dir, PathBuf::from("/srv/lethora"));
        assert_eq!(config.trusted_proxies.len(), 2);
        assert_eq!(config.limits.connections_per_ip, 7);
        assert_eq!(config.limits.project_bytes, 1_000_000);
        assert_eq!(config.limits.message_bytes, 2 << 20);
        assert_eq!(config.public_url.as_deref(), Some("https://sync.example"));

        for bad in [
            env(&[("LETHORA_CONNECTIONS_PER_IP", "many")]),
            env(&[("LETHORA_LISTEN", "nowhere")]),
            env(&[("LETHORA_CHUNK_BYTES", "0")]),
        ] {
            assert!(Config::load(None, bad).is_err());
        }
        std::fs::write(&path, "unknown = 1\n").unwrap();
        assert!(Config::load(Some(&path), []).is_err());
    }

    /// `deploy/lethora-server.example.toml` lists every key with its default
    /// value (only `public_url` is set), so it can't drift from the code.
    #[test]
    fn example_config() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/lethora-server.example.toml");
        let config = Config::load(Some(&path), []).unwrap();
        let expected = Config {
            public_url: Some("https://sync.example.org".into()),
            ..Config::default()
        };
        assert_eq!(config, expected);

        let text = std::fs::read_to_string(&path).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut all = TOP_LEVEL.to_vec();
        all.push("limits");
        all.sort_unstable();
        assert_eq!(keys, all);
        let limits = table["limits"].as_table().unwrap();
        let defaults = toml::Table::try_from(Limits::default()).unwrap();
        assert_eq!(
            limits.keys().collect::<Vec<_>>(),
            defaults.keys().collect::<Vec<_>>()
        );
    }
}
