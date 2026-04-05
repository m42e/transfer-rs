use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionMode {
    None,
    Passphrase,
    Identity,
}

impl EncryptionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Passphrase => "passphrase",
            Self::Identity => "identity",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "passphrase" => Self::Passphrase,
            "identity" => Self::Identity,
            _ => Self::None,
        }
    }

    pub fn is_encrypted(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl std::fmt::Display for EncryptionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRecord {
    pub id: String,
    pub original_name: String,
    pub remote_name: String,
    pub source_path: Option<String>,
    pub download_url: String,
    pub delete_url: String,
    pub uploaded_at: DateTime<Utc>,
    pub size_bytes: u64,
    pub encryption_mode: EncryptionMode,
    pub is_deleted: bool,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::{EncryptionMode, UploadRecord};
    use chrono::Utc;

    #[test]
    fn encryption_mode_string_conversions_are_stable() {
        assert_eq!(EncryptionMode::None.as_str(), "none");
        assert_eq!(EncryptionMode::Passphrase.as_str(), "passphrase");
        assert_eq!(EncryptionMode::Identity.as_str(), "identity");
        assert_eq!(EncryptionMode::from_db("passphrase"), EncryptionMode::Passphrase);
        assert_eq!(EncryptionMode::from_db("identity"), EncryptionMode::Identity);
        assert_eq!(EncryptionMode::from_db("anything-else"), EncryptionMode::None);
        assert!(!EncryptionMode::None.is_encrypted());
        assert!(EncryptionMode::Passphrase.is_encrypted());
        assert!(EncryptionMode::Identity.is_encrypted());
        assert_eq!(EncryptionMode::Identity.to_string(), "identity");
    }

    #[test]
    fn upload_record_round_trips_through_serde() {
        let record = UploadRecord {
            id: "id-1".to_owned(),
            original_name: "example.txt".to_owned(),
            remote_name: "example.txt.age".to_owned(),
            source_path: Some("/tmp/example.txt".to_owned()),
            download_url: "https://example.invalid/example.txt.age".to_owned(),
            delete_url: "https://example.invalid/delete/example.txt.age".to_owned(),
            uploaded_at: Utc::now(),
            size_bytes: 42,
            encryption_mode: EncryptionMode::Passphrase,
            is_deleted: true,
            deleted_at: Some(Utc::now()),
        };

        let encoded = serde_json::to_string(&record).expect("serialize upload record");
        let decoded: UploadRecord = serde_json::from_str(&encoded).expect("deserialize upload record");

        assert_eq!(decoded.id, record.id);
        assert_eq!(decoded.original_name, record.original_name);
        assert_eq!(decoded.remote_name, record.remote_name);
        assert_eq!(decoded.source_path, record.source_path);
        assert_eq!(decoded.download_url, record.download_url);
        assert_eq!(decoded.delete_url, record.delete_url);
        assert_eq!(decoded.size_bytes, record.size_bytes);
        assert_eq!(decoded.encryption_mode, record.encryption_mode);
        assert_eq!(decoded.is_deleted, record.is_deleted);
        assert_eq!(decoded.deleted_at, record.deleted_at);
    }
}