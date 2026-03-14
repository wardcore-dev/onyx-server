use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancesConfig {
    #[serde(rename = "instance")]
    pub instances: Vec<Instance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub config: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_restart: bool,
}

impl Default for Instance {
    fn default() -> Self {
        Self {
            name: String::new(),
            config: String::new(),
            port: None,
            enabled: true,
            auto_restart: false,
        }
    }
}

impl InstancesConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read instances config: {}", e))?;

        let config: Self = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse instances config: {}", e))?;

        // Validate unique instance names
        let mut names = std::collections::HashSet::new();
        for instance in &config.instances {
            if !names.insert(&instance.name) {
                return Err(format!("Duplicate instance name found: '{}'. All instance names must be unique.", instance.name));
            }
        }

        Ok(config)
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write config: {}", e))
    }

    pub fn find_instance(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    pub fn default_toml() -> String {
        r#"# ONYX Server - Process Manager Configuration
# This file defines multiple server instances that can be managed together

# Example instance configuration:
# [[instance]]
# name = "tech-chat"           # Unique instance name
# config = "configs/tech.toml" # Path to instance config file
# port = 3000                  # Override port (optional, will use port from config if not specified)
# enabled = true               # Enable this instance (default: true)
# auto_restart = false         # Auto-restart on crash (default: false)

[[instance]]
name = "main-group"
config = "config.toml"
port = 3000
enabled = true
auto_restart = false

# Uncomment to add more instances:
# [[instance]]
# name = "news-channel"
# config = "configs/news.toml"
# port = 3001
# enabled = true
# auto_restart = true

# [[instance]]
# name = "gaming-chat"
# config = "configs/gaming.toml"
# port = 3002
# enabled = true
# auto_restart = false
"#.to_string()
    }
}
