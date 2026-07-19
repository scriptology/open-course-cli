use std::path::PathBuf;

use open_course_cli::app::LlmResult;
use open_course_cli::config::read_config;
use open_course_cli::core::session::Exercise;
use open_course_cli::db::curriculum::{Difficulty, Topic};
use open_course_cli::llm::factory::create_llm_model;
use open_course_cli::llm::pipeline::generate_analysis;
use open_course_cli::llm::prompts::build_batch_analysis_prompt;
use tokio::sync::mpsc;

fn make_topic(id: &str, name: &str, difficulty: Difficulty, level: &str) -> Topic {
    Topic {
        id: id.to_string(),
        name: name.to_string(),
        description: format!("{name} description"),
        difficulty: difficulty.as_str().to_string(),
        level: Some(level.to_string()),
        order: None,
        tags: vec![],
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
    }
}

fn make_exercise(id: &str, target: &str, expected: &str) -> Exercise {
    Exercise {
        id: id.to_string(),
        target_sentence: target.to_string(),
        expected_translation: expected.to_string(),
        acceptable_translations: vec![],
        target_topic_ids: vec!["relative-pronouns".to_string()],
        side_topic_ids: vec![],
        expected_patterns: vec![],
        hint: None,
    }
}

#[tokio::test]
#[ignore = "makes a real LLM request"]
async fn debug_generate_analysis() {
    let data_dir = PathBuf::from(".");
    let config = read_config(&data_dir)
        .expect("failed to read config")
        .expect("no config");
    let model = create_llm_model(&config).expect("failed to create model");

    let exercises = vec![
        (
            make_exercise(
                "ex1",
                "The colleague who joined our team believes that knowledge is more important than experience.",
                "Коллега, который присоединился к нашей команде, считает, что знания важнее опыта.",
            ),
            "Collegue, which joined to our team, thinks, that knowledge more important than expirience".to_string(),
        ),
        (
            make_exercise(
                "ex2",
                "Please save these files to an external drive.",
                "Пожалуйста, сохраните эти файлы на внешний диск.",
            ),
            "Please save these documents on the external disk.".to_string(),
        ),
    ];

    let candidate_topics = vec![
        make_topic(
            "relative-pronouns",
            "Relative Pronouns",
            Difficulty::Beginner,
            "A2",
        ),
        make_topic(
            "prepositions-save",
            "Prepositions with Save",
            Difficulty::Beginner,
            "A2",
        ),
        make_topic("spelling", "Spelling", Difficulty::Beginner, "A2"),
    ];

    let prompt =
        build_batch_analysis_prompt(config.active_profile(), &exercises, &candidate_topics);

    println!("\n========== PROMPT ==========\n{prompt}\n===========================\n");

    let (tx, mut rx) = mpsc::channel::<LlmResult>(16);
    let stream_handle = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    match generate_analysis(model.as_ref(), &prompt, 1, Some(&tx), Some(&data_dir)).await {
        Ok(analysis) => {
            drop(tx);
            let _ = stream_handle.await;
            println!("Analysis parsed successfully:");
            println!("{analysis:#?}");
        }
        Err(e) => {
            drop(tx);
            let _ = stream_handle.await;
            eprintln!("ERROR: {e}");
            panic!("analysis generation failed");
        }
    }
}
