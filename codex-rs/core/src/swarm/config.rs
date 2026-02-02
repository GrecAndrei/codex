use crate::config::types::SwarmHierarchyToml;
use crate::config::types::SwarmHubToml;
use crate::config::types::SwarmToml;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmConfig {
    pub enabled: bool,
    pub root_role: Option<String>,
    pub default_spawn_role: Option<String>,
    pub roles: Vec<SwarmRole>,
    pub hierarchy: SwarmHierarchy,
    pub hub: SwarmHubConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmRole {
    pub name: String,
    pub model: Option<String>,
    pub base_instructions: Option<String>,
    pub tier: i32,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwarmHierarchy {
    pub allow_upward_calls: bool,
    pub allow_same_tier_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SwarmHubConfig {
    pub leak_tracker_path: Option<PathBuf>,
    pub storage_dir: Option<PathBuf>,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            root_role: Some("Scholar".to_string()),
            default_spawn_role: Some("Scribe".to_string()),
            roles: default_roles(),
            hierarchy: SwarmHierarchy::default(),
            hub: SwarmHubConfig::default(),
        }
    }
}

impl Default for SwarmHierarchy {
    fn default() -> Self {
        Self {
            allow_upward_calls: false,
            allow_same_tier_calls: true,
        }
    }
}

impl SwarmConfig {
    pub fn from_toml(toml: Option<SwarmToml>) -> Self {
        let mut config = Self::default();
        let Some(toml) = toml else {
            return config;
        };

        if let Some(enabled) = toml.enabled {
            config.enabled = enabled;
        }
        if let Some(root_role) = toml.root_role {
            if !root_role.trim().is_empty() {
                config.root_role = Some(root_role);
            }
        }
        if let Some(default_spawn_role) = toml.default_spawn_role {
            if !default_spawn_role.trim().is_empty() {
                config.default_spawn_role = Some(default_spawn_role);
            }
        }
        if let Some(roles) = toml.roles {
            let mut converted = Vec::new();
            for (idx, role) in roles.into_iter().enumerate() {
                if role.name.trim().is_empty() {
                    continue;
                }
                let tier = role
                    .tier
                    .unwrap_or_else(|| i32::try_from(idx).unwrap_or(i32::MAX));
                converted.push(SwarmRole {
                    name: role.name,
                    model: role.model,
                    base_instructions: role.base_instructions,
                    tier,
                    description: role.description,
                });
            }
            if !converted.is_empty() {
                config.roles = converted;
            }
        }
        if let Some(hierarchy) = toml.hierarchy {
            config.hierarchy = SwarmHierarchy::from_toml(&hierarchy, config.hierarchy);
        }
        if let Some(hub) = toml.hub {
            config.hub = SwarmHubConfig::from_toml(&hub);
        }

        config
    }

    pub fn role(&self, name: &str) -> Option<&SwarmRole> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.roles
            .iter()
            .find(|role| role.name.eq_ignore_ascii_case(name))
    }

    pub fn root_role_name(&self) -> Option<&str> {
        self.root_role
            .as_deref()
            .and_then(|name| self.role(name).map(|_| name))
            .or_else(|| {
                self.roles
                    .iter()
                    .max_by_key(|role| role.tier)
                    .map(|role| role.name.as_str())
            })
    }

    pub fn default_spawn_role_name(&self) -> Option<&str> {
        if let Some(default_spawn_role) = self
            .default_spawn_role
            .as_deref()
            .and_then(|name| self.role(name).map(|_| name))
        {
            return Some(default_spawn_role);
        }
        self.roles.first().map(|role| role.name.as_str())
    }

    pub fn can_call(&self, caller_tier: i32, target_tier: i32) -> bool {
        if caller_tier == target_tier {
            return self.hierarchy.allow_same_tier_calls;
        }
        if caller_tier > target_tier {
            return true;
        }
        self.hierarchy.allow_upward_calls
    }
}

impl SwarmHierarchy {
    fn from_toml(toml: &SwarmHierarchyToml, base: Self) -> Self {
        Self {
            allow_upward_calls: toml.allow_upward_calls.unwrap_or(base.allow_upward_calls),
            allow_same_tier_calls: toml
                .allow_same_tier_calls
                .unwrap_or(base.allow_same_tier_calls),
        }
    }
}

impl SwarmHubConfig {
    fn from_toml(toml: &SwarmHubToml) -> Self {
        Self {
            leak_tracker_path: toml
                .leak_tracker_path
                .as_ref()
                .map(|p| p.as_path().to_path_buf()),
            storage_dir: toml.storage_dir.as_ref().map(|p| p.as_path().to_path_buf()),
        }
    }
}

fn default_roles() -> Vec<SwarmRole> {
    vec![
        SwarmRole {
            name: "Scout".to_string(),
            model: Some("gpt-5.1-codex-mini".to_string()),
            base_instructions: None,
            tier: 0,
            description: Some("High-throughput triage and acquisition.".to_string()),
        },
        SwarmRole {
            name: "Scribe".to_string(),
            model: Some("gpt-5.1-codex-max".to_string()),
            base_instructions: None,
            tier: 1,
            description: Some("Structural mapping and deep audit.".to_string()),
        },
        SwarmRole {
            name: "Scholar".to_string(),
            model: Some("gpt-5.2-codex".to_string()),
            base_instructions: None,
            tier: 2,
            description: Some("High-reasoning synthesis and strategy.".to_string()),
        },
    ]
}
