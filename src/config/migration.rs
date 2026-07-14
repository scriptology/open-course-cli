use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::open_course_dir;
use crate::config::pair_db_path;
use crate::config::profile::{LanguagePair, UserProfile};
use crate::config::{OpenCourseConfig, ProviderConfig, ProviderId};
use crate::db::curriculum::Curriculum;
use crate::error::Result;

pub fn try_migrate_from_profile_md(
    cwd: &std::path::Path,
) -> Result<Option<(OpenCourseConfig, PathBuf)>> {
    let legacy_path = open_course_dir(cwd).join("profile.md");
    if !legacy_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&legacy_path)?;
    let profile = parse_legacy_profile(&content);
    let config = OpenCourseConfig::new(
        ProviderId::Custom,
        ProviderConfig::ApiKey {
            model: String::new(),
            api_key: None,
            base_url: None,
            endpoint: None,
            reasoning_effort: None,
        },
        profile,
    );
    Ok(Some((config, legacy_path)))
}

fn parse_legacy_profile(content: &str) -> UserProfile {
    let mut meta: HashMap<String, String> = HashMap::new();
    if let Some(frontmatter) = content
        .strip_prefix("---\n")
        .and_then(|s| s.split("\n---\n").next())
    {
        for line in frontmatter.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            meta.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    UserProfile {
        native_language: meta
            .get("nativeLanguage")
            .cloned()
            .unwrap_or_else(|| "en".to_string()),
        target_language: meta
            .get("targetLanguage")
            .cloned()
            .unwrap_or_else(|| "en".to_string()),
        age: meta.get("age").and_then(|v| v.parse().ok()),
        self_assessed_cefr: meta
            .get("selfAssessedCefr")
            .or_else(|| meta.get("cefr"))
            .cloned(),
    }
}

pub fn migrate_legacy_config(cwd: &std::path::Path, config: &mut OpenCourseConfig) -> Result<bool> {
    strip_opencode_model_prefixes(config);
    if config.version >= 2 {
        return Ok(false);
    }
    if let Some(profile) = config.profile.take() {
        let id = OpenCourseConfig::pair_id(&profile.native_language, &profile.target_language);
        config.pairs.push(LanguagePair {
            id: id.clone(),
            profile,
        });
        config.active_pair = id.clone();

        let old_db = open_course_dir(cwd).join("db");
        let new_db = pair_db_path(cwd, &id);
        if old_db.exists() && !new_db.exists() {
            std::fs::create_dir_all(new_db.parent().unwrap())?;
            std::fs::rename(&old_db, &new_db)?;
        }
    }
    if config.active_pair.is_empty() && !config.pairs.is_empty() {
        config.active_pair = config.pairs[0].id.clone();
    }
    config.version = 2;
    Ok(true)
}

fn strip_opencode_model_prefixes(config: &mut OpenCourseConfig) {
    if let Some(ProviderConfig::ApiKey { model, .. }) =
        config.providers.get_mut(&config.active_provider)
        && model.starts_with("opencode/")
    {
        *model = model.trim_start_matches("opencode/").to_string();
    }
}

pub fn try_migrate_from_curriculum_md(cwd: &std::path::Path) -> Result<Option<Curriculum>> {
    let legacy_path = open_course_dir(cwd).join("curriculum.md");
    if !legacy_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&legacy_path)?;
    let Some(json) = extract_curriculum_json(&content) else {
        return Ok(None);
    };
    let mut curriculum: Curriculum = serde_json::from_str(&json)?;
    for topic in &mut curriculum.topics {
        if topic.level.is_none() {
            topic.level = crate::db::curriculum::difficulty_to_cefr(&topic.difficulty);
        }
        if topic.order.is_none() {
            topic.order = topic.cefr_numeric().into();
        }
    }
    let backup_path = legacy_path.with_extension("md.backup");
    std::fs::rename(&legacy_path, backup_path)?;
    Ok(Some(curriculum))
}

pub fn should_recreate_curriculum_table(cwd: &std::path::Path) -> bool {
    let marker_path = open_course_dir(cwd).join(".curriculum_table_v2_recreated");
    !marker_path.exists()
}

pub fn mark_curriculum_table_recreated(cwd: &std::path::Path) -> Result<()> {
    let marker_path = open_course_dir(cwd).join(".curriculum_table_v2_recreated");
    std::fs::write(&marker_path, "")?;
    Ok(())
}

fn extract_curriculum_json(content: &str) -> Option<String> {
    let start = content.find("<curriculum>")? + "<curriculum>".len();
    let end = content.find("</curriculum>")?;
    if end <= start {
        return None;
    }
    Some(content[start..end].trim().to_string())
}

pub fn should_clear_reviews_cache(cwd: &std::path::Path) -> bool {
    let marker_path = open_course_dir(cwd).join(".reviews_cache_v3_no_tables");
    !marker_path.exists()
}

pub fn mark_reviews_cache_cleared(cwd: &std::path::Path) -> Result<()> {
    let marker_path = open_course_dir(cwd).join(".reviews_cache_v3_no_tables");
    std::fs::write(&marker_path, "")?;
    Ok(())
}
