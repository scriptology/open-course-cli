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
        level: Some("A2".to_string()),
        order: None,
        tags: vec![],
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
        ..Default::default()
    }
}

#[test]
fn exercise_prompt_includes_profile() {
    let p = profile();
    let all = vec![topic("t1"), topic("t2")];
    let target = vec![topic("t1")];
    let side = vec![topic("t2")];
    let prompt = build_exercise_prompt(&p, &target, &side, &all, &[], &[], 3, 0.75);

    // Prompt prose spells out the language name for the LLM (not the raw
    // "ru"/"en" codes `UserProfile` stores internally) — see `english_name`
    // in `crates/core/src/language.rs`.
    assert!(prompt.contains("Russian to English"));
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
    // The topic's CEFR level anchors complexity, overriding the profile's B1.
    assert!(prompt.contains("Topic t1 (difficulty: beginner, CEFR: A2)"));
    assert!(prompt.contains("complexity MUST match CEFR level A2"));
    assert!(prompt.contains("6-12 words per sentence"));
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
        ..Default::default()
    }];
    let prompt = build_exercise_prompt(&p, &target, &[], &all, &items, &[], 3, 0.75);

    assert!(prompt.contains("learning items"));
    assert!(prompt.contains("a/an"));
    assert!(prompt.contains("articles"));
}

#[test]
fn exercise_prompt_includes_forced_vocabulary() {
    use open_course_cli::core::vocabulary::Lemma;

    let p = profile();
    let all = vec![topic("t1")];
    let target = vec![topic("t1")];
    let lemmas = vec![
        Lemma {
            id: "en-colleague".to_string(),
            lemma: "colleague".to_string(),
            pos: "NOUN".to_string(),
            target_lang: "en".to_string(),
            native_lang: "ru".to_string(),
            translation: "коллега".to_string(),
            ..Default::default()
        },
        Lemma {
            id: "en-resilient".to_string(),
            lemma: "resilient".to_string(),
            pos: "ADJ".to_string(),
            target_lang: "en".to_string(),
            native_lang: "ru".to_string(),
            ..Default::default()
        },
    ];
    let prompt = build_exercise_prompt(&p, &target, &[], &all, &[], &lemmas, 3, 0.75);

    assert!(prompt.contains("words need extra practice"));
    assert!(prompt.contains("- colleague (коллега)"));
    assert!(prompt.contains("- resilient"));
    // Forced vocabulary also triggers the warm-up contract.
    assert!(prompt.contains("\"warmup\""));
    assert!(prompt.contains("one entry per word listed above"));
}

#[test]
fn exercise_prompt_always_requests_vocabulary_extraction() {
    let p = profile();
    let all = vec![topic("t1")];
    let target = vec![topic("t1")];
    // No forced learning items, no forced vocabulary: the "vocabulary"
    // extraction request must still be present, independent of forced
    // vocabulary — it's how genuinely new words get previewed.
    let prompt = build_exercise_prompt(&p, &target, &[], &all, &[], &[], 3, 0.75);

    assert!(prompt.contains("\"vocabulary\""));
    assert!(prompt.contains("CONTENT word"));
    assert!(!prompt.contains("\"warmup\""));
}

#[test]
fn exercise_prompt_anchors_vocabulary_extraction_to_expected_translation() {
    let p = profile();
    let all = vec![topic("t1")];
    let target = vec![topic("t1")];
    let prompt = build_exercise_prompt(&p, &target, &[], &all, &[], &[], 3, 0.75);

    // `targetSentence` holds the native-language sentence; the vocabulary
    // instructions must name `expectedTranslation` explicitly so the model
    // extracts target-language words instead.
    assert!(prompt.contains("appears in the expectedTranslation fields"));
    assert!(prompt.contains("never extract words from targetSentence"));
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
    assert!(prompt.contains("explanations and comments must be in Russian"));
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
