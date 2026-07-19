use open_course_cli::config::profile::UserProfile;
use open_course_cli::db::curriculum::{Difficulty, Topic};
use open_course_cli::db::learning_items::LearningItem;
use open_course_cli::llm::prompts::{
    build_batch_analysis_prompt, build_curriculum_level_prompt, build_exercise_prompt,
    build_topic_review_prompt,
};

fn profile() -> UserProfile {
    UserProfile {
        native_language: "ru".to_string(),
        target_language: "en".to_string(),
        age: Some(30),
        self_assessed_cefr: Some("B1".to_string()),
    }
}

fn topic(id: &str) -> Topic {
    Topic {
        id: id.to_string(),
        name: format!("Topic {id}"),
        description: "desc".to_string(),
        difficulty: Difficulty::Beginner.as_str().to_string(),
        level: None,
        order: None,
        tags: vec![],
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
    }
}

#[test]
fn exercise_prompt_includes_profile() {
    let p = profile();
    let all = vec![topic("t1"), topic("t2")];
    let target = vec![topic("t1")];
    let side = vec![topic("t2")];
    let prompt = build_exercise_prompt(&p, &target, &side, &all, &[], 3, 0.75);

    assert!(prompt.contains("ru to en"));
    assert!(prompt.contains("Target topics: Topic t1"));
    assert!(prompt.contains("B1"));
    assert!(prompt.contains("Student age: 30"));
    assert!(
        prompt.contains(
            "contexts and examples that fit the life experience of a typical 30-year-old"
        )
    );
    assert!(prompt.contains("topicId: \"t1\""));
    assert!(prompt.contains("JSON object"));
    assert!(prompt.contains("exercises"));
}

#[test]
fn exercise_prompt_includes_forced_learning_items() {
    let p = profile();
    let all = vec![topic("t1")];
    let target = vec![topic("t1")];
    let items = vec![LearningItem {
        id: "en-grammar".to_string(),
        name: "a/an".to_string(),
        description: "articles".to_string(),
        level: None,
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        score: 0.0,
        last_practiced: None,
        practice_count: 0,
    }];
    let prompt = build_exercise_prompt(&p, &target, &[], &all, &items, 3, 0.75);

    assert!(prompt.contains("learning items"));
    assert!(prompt.contains("a/an"));
    assert!(prompt.contains("articles"));
}

#[test]
fn analysis_prompt_includes_answers() {
    use open_course_cli::core::session::Exercise;

    let p = profile();
    let exercise = Exercise {
        id: "e1".to_string(),
        target_sentence: "Hello".to_string(),
        expected_translation: "Привет".to_string(),
        acceptable_translations: vec![],
        target_topic_ids: vec!["t1".to_string()],
        side_topic_ids: vec![],
        expected_patterns: vec![],
        hint: None,
    };
    let pairs = vec![(exercise, "Hi".to_string())];
    let topics = vec![topic("t1")];
    let prompt = build_batch_analysis_prompt(&p, &pairs, &topics);

    assert!(prompt.contains("Student translation: Hi"));
    assert!(prompt.contains("topicId: \"t1\""));
    assert!(prompt.contains("explanations and comments must be in ru"));
    assert!(prompt.contains("\"spelling\""));
    assert!(prompt.contains("Do NOT include `newTopics` for spelling errors"));
    assert!(prompt.contains("generalizable, reusable grammar or usage pattern"));
    assert!(prompt.contains("vocabulary review item instead of a topic"));
}

#[test]
fn topic_review_prompt_includes_topic_name() {
    let p = profile();
    let t = topic("greetings");
    let prompt = build_topic_review_prompt(&p, &t);

    assert!(prompt.contains("greetings"));
    assert!(prompt.contains("superpowers"));
}

#[test]
fn curriculum_level_prompt_includes_domains_and_level() {
    let p = profile();
    let prompt = build_curriculum_level_prompt(&p, "B1", Some("A2"), 12);

    assert!(prompt.contains("A2"));
    assert!(prompt.contains("B1"));
    assert!(prompt.contains("targetLanguage"));
    assert!(prompt.contains("nativeLanguage"));
    assert!(prompt.contains("lexicon-vocabulary"));
    assert!(prompt.contains("domain:"));
}
