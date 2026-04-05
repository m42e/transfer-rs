use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::storage::paths::AppPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server_url: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: "https://transfer.pb42.de".to_owned(),
        }
    }
}

impl AppConfig {
    pub fn load_or_create(paths: &AppPaths) -> Result<Self> {
        if !paths.config_file.exists() {
            let config = Self::default();
            config.save(paths)?;
            return Ok(config);
        }

        let contents = std::fs::read_to_string(&paths.config_file)
            .with_context(|| format!("failed to read {}", paths.config_file.display()))?;
        toml::from_str(&contents).context("failed to parse config file")
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let contents = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(&paths.config_file, contents)
            .with_context(|| format!("failed to write {}", paths.config_file.display()))
    }

    pub fn resolve_server_url(&self, override_url: Option<&str>) -> String {
        override_url.unwrap_or(&self.server_url).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use tempfile::{TempDir, tempdir};

    fn test_paths() -> Result<(TempDir, AppPaths)> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok((root, AppPaths::from_dirs(config_dir, data_dir)))
    }

    #[test]
    fn default_server_url_matches_expected_endpoint() {
        assert_eq!(AppConfig::default().server_url, "https://transfer.pb42.de");
    }

    #[test]
    fn load_or_create_creates_default_config_when_missing() -> Result<()> {
        let (_root, paths) = test_paths()?;

        let config = AppConfig::load_or_create(&paths)?;

        assert_eq!(config.server_url, AppConfig::default().server_url);
        assert!(paths.config_file.exists());
        Ok(())
    }

    #[test]
    fn save_and_load_round_trip() -> Result<()> {
        let (_root, paths) = test_paths()?;
        let config = AppConfig {
            server_url: "https://example.invalid".to_owned(),
        };

        config.save(&paths)?;
        let loaded = AppConfig::load_or_create(&paths)?;

        assert_eq!(loaded.server_url, config.server_url);
        Ok(())
    }

    #[test]
    fn load_or_create_rejects_invalid_toml() -> Result<()> {
        let (_root, paths) = test_paths()?;
        std::fs::write(&paths.config_file, "not = [valid")?;

        let error = AppConfig::load_or_create(&paths).expect_err("invalid config should fail");

        assert!(error.to_string().contains("failed to parse config file"));
        Ok(())
    }

    #[test]
    fn resolve_server_url_prefers_override() {
        let config = AppConfig {
            server_url: "https://stored.invalid".to_owned(),
        };

        assert_eq!(
            config.resolve_server_url(Some("https://override.invalid")),
            "https://override.invalid"
        );
        assert_eq!(config.resolve_server_url(None), "https://stored.invalid");
    }
}
