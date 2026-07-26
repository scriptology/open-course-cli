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
    assert_eq!(
        config.active_profile().self_assessed_cefr,
        Some("A2".to_string())
    );
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
    assert_eq!(
        fs::read_to_string(new_db.join("marker.txt")).unwrap(),
        "data"
    );
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

#[test]
fn resolve_data_dir_prefers_global_home() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    // Local config exists, global does not: local data is migrated to global.
    let local_dir = cwd.path().join(".open-course-cli");
    fs::create_dir_all(&local_dir).unwrap();
    fs::write(local_dir.join("config.json"), "{}").unwrap();
    fs::write(local_dir.join("marker.txt"), "data").unwrap();

    let resolved = config::resolve_data_dir_with_home(cwd.path(), Some(home.path()));
    assert_eq!(resolved, home.path());

    let global_dir = home.path().join(".open-course-cli");
    assert!(global_dir.join("config.json").exists());
    assert!(global_dir.join("marker.txt").exists());
    assert!(!local_dir.exists());
}

#[test]
fn resolve_data_dir_keeps_global_when_it_already_has_config() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let local_dir = cwd.path().join(".open-course-cli");
    fs::create_dir_all(&local_dir).unwrap();
    fs::write(local_dir.join("config.json"), "{\"local\":true}").unwrap();
    let global_dir = home.path().join(".open-course-cli");
    fs::create_dir_all(&global_dir).unwrap();
    fs::write(global_dir.join("config.json"), "{\"global\":true}").unwrap();

    let resolved = config::resolve_data_dir_with_home(cwd.path(), Some(home.path()));
    assert_eq!(resolved, home.path());
    // Global config is untouched, local stays in place.
    assert_eq!(
        fs::read_to_string(global_dir.join("config.json")).unwrap(),
        "{\"global\":true}"
    );
    assert!(local_dir.join("config.json").exists());
}

#[test]
fn resolve_data_dir_returns_home_for_fresh_install() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let resolved = config::resolve_data_dir_with_home(cwd.path(), Some(home.path()));
    assert_eq!(resolved, home.path());
}

#[test]
fn resolve_data_dir_falls_back_to_cwd_without_home() {
    let cwd = TempDir::new().unwrap();
    let resolved = config::resolve_data_dir_with_home(cwd.path(), None);
    assert_eq!(resolved, cwd.path());
}

#[test]
fn resolve_data_dir_merges_into_existing_global_without_config() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    // Global dir exists (e.g. fallback db) but has no config yet.
    let global_dir = home.path().join(".open-course-cli");
    fs::create_dir_all(global_dir.join("db")).unwrap();
    // Local dir has a config plus an entry that conflicts with global.
    let local_dir = cwd.path().join(".open-course-cli");
    fs::create_dir_all(local_dir.join("db")).unwrap();
    fs::write(local_dir.join("config.json"), "{}").unwrap();

    let resolved = config::resolve_data_dir_with_home(cwd.path(), Some(home.path()));
    assert_eq!(resolved, home.path());
    assert!(global_dir.join("config.json").exists());
    // Conflicting entry is not overwritten; local leftovers stay in place.
    assert!(local_dir.join("db").exists());
    assert!(!local_dir.join("config.json").exists());
}
