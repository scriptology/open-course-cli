use std::fs;
use tempfile::TempDir;

use open_course_cli::config::profile::UserProfile;
use open_course_cli::config::{
    self, OpenCourseConfig, ProviderConfig, ProviderId, read_config, write_config,
};

#[test]
fn read_config_returns_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let result = read_config(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn config_roundtrip() {
    let dir = TempDir::new().unwrap();
    let profile = UserProfile {
        native_language: "ru".to_string(),
        target_language: "en".to_string(),
        age: Some(30),
        self_assessed_cefr: Some("B1".to_string()),
    };
    let provider_config = ProviderConfig::ApiKey {
        api_key: Some("test-key".to_string()),
        model: "gpt-4".to_string(),
        base_url: None,
        endpoint: None,
        reasoning_effort: None,
    };
    let config = OpenCourseConfig::new(ProviderId::OpenAi, provider_config, profile);
    write_config(&config, dir.path()).unwrap();

    let read = read_config(dir.path()).unwrap().unwrap();
    assert_eq!(read, config);
    assert!(config::has_config(dir.path()));
}

#[test]
fn legacy_profile_migration() {
    let dir = TempDir::new().unwrap();
    let open_course_dir = dir.path().join(".open-course-cli");
    fs::create_dir_all(&open_course_dir).unwrap();
    let profile_md = open_course_dir.join("profile.md");
    fs::write(
        &profile_md,
        "---\nnativeLanguage: ru\ntargetLanguage: en\nage: 25\nselfAssessedCefr: A2\n---\n",
    )
    .unwrap();

    let config = read_config(dir.path()).unwrap().unwrap();
    assert_eq!(config.active_profile().native_language, "ru");
    assert_eq!(config.active_profile().target_language, "en");
    assert_eq!(config.active_profile().age, Some(25));
    assert_eq!(config.active_profile().self_assessed_cefr, Some("A2".to_string()));
    assert_eq!(config.active_pair, "ru-en");
    assert_eq!(config.pairs.len(), 1);
    assert_eq!(config.active_provider, ProviderId::Custom);
    assert!(!profile_md.exists());
}

#[test]
fn v1_config_to_pairs_migration_moves_db() {
    use open_course_cli::config::pair_db_path;

    let dir = TempDir::new().unwrap();
    let open_course_dir = dir.path().join(".open-course-cli");
    fs::create_dir_all(&open_course_dir).unwrap();
    let legacy_json = r#"{
        "version": 1,
        "activeProvider": "openai",
        "providers": {
            "openai": {
                "type": "apiKey",
                "model": "gpt-4",
                "apiKey": null,
                "baseUrl": null
            }
        },
        "profile": {
            "nativeLanguage": "ru",
            "targetLanguage": "es",
            "age": null,
            "selfAssessedCefr": null
        },
        "preferences": {}
    }"#;
    fs::write(open_course_dir.join("config.json"), legacy_json).unwrap();
    let old_db = open_course_dir.join("db");
    fs::create_dir_all(&old_db).unwrap();
    fs::write(old_db.join("marker.txt"), "data").unwrap();

    let config = read_config(dir.path()).unwrap().unwrap();
    assert_eq!(config.version, 2);
    assert_eq!(config.active_pair, "ru-es");
    assert_eq!(config.pairs.len(), 1);

    let new_db = pair_db_path(dir.path(), "ru-es");
    assert!(new_db.join("marker.txt").exists());
    assert_eq!(fs::read_to_string(new_db.join("marker.txt")).unwrap(), "data");
}

#[test]
fn opencode_to_custom_migration() {
    let dir = TempDir::new().unwrap();
    let legacy_json = r#"{
        "version": 1,
        "activeProvider": "opencode",
        "providers": {
            "opencode": {
                "type": "openCode",
                "model": "opencode/claude-sonnet",
                "apiKey": null,
                "baseUrl": null
            }
        },
        "profile": {
            "nativeLanguage": "en",
            "targetLanguage": "es",
            "age": null,
            "selfAssessedCefr": null
        },
        "preferences": {}
    }"#;
    let open_course_dir = dir.path().join(".open-course-cli");
    fs::create_dir_all(&open_course_dir).unwrap();
    fs::write(open_course_dir.join("config.json"), legacy_json).unwrap();

    let read = read_config(dir.path()).unwrap().unwrap();
    assert_eq!(read.active_provider, ProviderId::Custom);
    assert_eq!(read.active_pair, "en-es");
    assert_eq!(read.pairs.len(), 1);
    let provider = read.providers.get(&ProviderId::Custom).unwrap();
    assert_eq!(provider.model(), "claude-sonnet");
}
