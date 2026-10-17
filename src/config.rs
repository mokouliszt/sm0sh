use serde::Deserialize;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bind: SocketAddr,
    pub transport: Transport,
    pub allowed_ips: Vec<IpAddr>,
    pub d_words: usize,
    pub w_words: usize,
    pub sd_words: usize,
    pub command_timeout_ms: u64,
    pub connection_timeout_ms: u64,
    pub max_connections: usize,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Tcp,
    Udp,
    Both,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:5000".parse().unwrap(),
            transport: Transport::Tcp,
            allowed_ips: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
            d_words: 8192,
            w_words: 128,
            sd_words: 8192,
            command_timeout_ms: 30_000,
            connection_timeout_ms: 30_000,
            max_connections: 16,
            working_directory: PathBuf::from("."),
            environment: BTreeMap::from([
                ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
                ("LANG".into(), "C.UTF-8".into()),
            ]),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let config: Self =
            toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=65535).contains(&self.d_words) {
            return Err("d_words must be 1..65535".into());
        }
        if !(2..=1024).contains(&self.w_words) {
            return Err("w_words must be 2..1024".into());
        }
        if !(1..=65535).contains(&self.sd_words) {
            return Err("sd_words must be 1..65535".into());
        }
        if !(1..=86_400_000).contains(&self.command_timeout_ms) {
            return Err("command_timeout_ms must be 1..86400000".into());
        }
        if !(100..=3_600_000).contains(&self.connection_timeout_ms) {
            return Err("connection_timeout_ms must be 100..3600000".into());
        }
        if !(1..=1024).contains(&self.max_connections) {
            return Err("max_connections must be 1..1024".into());
        }
        if self.allowed_ips.is_empty() {
            return Err("allowed_ips must list at least one PLC address".into());
        }
        if !self.working_directory.is_dir() {
            return Err("working_directory is not a directory".into());
        }
        for (key, value) in &self.environment {
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err("invalid environment entry".into());
            }
        }
        Ok(())
    }

    pub fn command_timeout(&self) -> Duration {
        Duration::from_millis(self.command_timeout_ms)
    }
    pub fn connection_timeout(&self) -> Duration {
        Duration::from_millis(self.connection_timeout_ms)
    }
    pub fn allows(&self, ip: IpAddr) -> bool {
        let normalized = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            _ => ip,
        };
        self.allowed_ips.contains(&normalized)
    }
}
