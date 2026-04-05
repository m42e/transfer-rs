use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub db_path: PathBuf,
    pub identity_path: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("de", "pb42", "transfer-rs")
            .context("failed to resolve application directories")?;
        let config_dir = project_dirs.config_dir().to_path_buf();
        let data_dir = project_dirs.data_local_dir().to_path_buf();
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("failed to create {}", config_dir.display()))?;
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create {}", data_dir.display()))?;

        Ok(Self {
            config_file: config_dir.join("config.toml"),
            db_path: data_dir.join("history.sqlite3"),
            identity_path: data_dir.join("identity.agekey"),
            config_dir,
            data_dir,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_dirs(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            config_file: config_dir.join("config.toml"),
            db_path: data_dir.join("history.sqlite3"),
            identity_path: data_dir.join("identity.agekey"),
            config_dir,
            data_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppPaths;
    use anyhow::Result;
    use serial_test::serial;
    use tempfile::tempdir;

    #[test]
    fn from_dirs_builds_expected_paths() {
        let config_dir = std::path::PathBuf::from("/tmp/config-root");
        let data_dir = std::path::PathBuf::from("/tmp/data-root");

        let paths = AppPaths::from_dirs(config_dir.clone(), data_dir.clone());

        assert_eq!(paths.config_dir, config_dir);
        assert_eq!(paths.data_dir, data_dir);
        assert_eq!(paths.config_file, std::path::PathBuf::from("/tmp/config-root/config.toml"));
        assert_eq!(paths.db_path, std::path::PathBuf::from("/tmp/data-root/history.sqlite3"));
        assert_eq!(paths.identity_path, std::path::PathBuf::from("/tmp/data-root/identity.agekey"));
    }

    #[test]
    #[serial]
    fn discover_creates_application_directories() -> Result<()> {
        let home = tempdir()?;
        let previous_home = std::env::var_os("HOME");

        unsafe {
            std::env::set_var("HOME", home.path());
        }

        let result = AppPaths::discover();

        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let paths = result?;
        assert!(paths.config_dir.exists());
        assert!(paths.data_dir.exists());
        assert!(paths.config_file.ends_with("config.toml"));
        assert!(paths.db_path.ends_with("history.sqlite3"));
        assert!(paths.identity_path.ends_with("identity.agekey"));
        Ok(())
    }

    #[test]
    #[serial]
    fn discover_restores_missing_home_environment() -> Result<()> {
        let home = tempdir()?;
        let saved_home = std::env::var_os("HOME");
        unsafe {
            std::env::remove_var("HOME");
        }
        let previous_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home.path());
        }

        let result = AppPaths::discover();

        unsafe {
            std::env::remove_var("HOME");
        }
        match saved_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(previous_home.is_none());

        let paths = result?;
        assert!(paths.config_dir.exists());
        assert!(paths.data_dir.exists());
        Ok(())
    }
}