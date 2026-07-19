use std::path::PathBuf;

use open_course_cli::config::read_config;
use open_course_cli::db::curriculum::{Difficulty, Topic};
use open_course_cli::llm::factory::create_llm_model;
use open_course_cli::llm::pipeline::generate_exercises;
use open_course_cli::llm::prompts::build_exercise_prompt;

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

#[tokio::test]
#[ignore = "makes a real LLM request"]
async fn debug_generate_exercises() {
    let data_dir = PathBuf::from(".");
    let config = read_config(&data_dir)
        .expect("failed to read config")
        .expect("no config");
    let model = create_llm_model(&config).expect("failed to create model");

    let target_topic = make_topic(
        "present-simple",
        "Present Simple",
        Difficulty::Beginner,
        "A2",
    );
    let side_topic = make_topic("word-order", "Word Order", Difficulty::Beginner, "A2");
    let candidate_topics = vec![
        target_topic.clone(),
        side_topic.clone(),
        make_topic(
            "articles-basic",
            "Basic Articles",
            Difficulty::Beginner,
            "A2",
        ),
    ];

    let prompt = build_exercise_prompt(
        config.active_profile(),
        &[target_topic],
        &[side_topic],
        &candidate_topics,
        &[],
        3,
        0.75,
    );

    println!("\n========== PROMPT ==========\n{prompt}\n===========================\n");

    match generate_exercises(model.as_ref(), &prompt, None, Some(&data_dir)).await {
        Ok(exercises) => {
            println!("Generated {} exercises:", exercises.len());
            for ex in exercises {
                println!("{ex:?}");
            }
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            panic!("exercise generation failed");
        }
    }
}
