use crate::config::profile::UserProfile;
use crate::core::session::{Exercise, NewTopicRef};
use crate::db::curriculum::{CURRICULUM_DOMAIN_DESCRIPTIONS, Topic, cefr_to_difficulty};
use crate::db::progress::ProgressTopic;

pub fn build_exercise_prompt(
    profile: &UserProfile,
    target_topics: &[Topic],
    side_topics: &[Topic],
    candidate_topics: &[Topic],
    count: u32,
) -> String {
    let target_names = target_topics
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let target_names = if target_names.is_empty() {
        "(no specific topics yet)".to_string()
    } else {
        target_names
    };

    let side_names = side_topics
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let side_names = if side_names.is_empty() {
        "(none)".to_string()
    } else {
        side_names
    };

    let cefr_hint = profile
        .self_assessed_cefr
        .as_ref()
        .map(|c| format!("Proficiency level (self-assessed): {c}"))
        .unwrap_or_default();

    let age_hint = profile
        .age
        .map(|age| format!(
            "Student age: {age}. Use age-appropriate contexts. Avoid school, kindergarten, or other child-specific scenarios unless the age makes them clearly relevant."
        ))
        .unwrap_or_default();

    let difficulty_hint = if target_topics.is_empty() {
        "general".to_string()
    } else {
        target_topics
            .iter()
            .map(|t| format!("{} ({})", t.name, t.difficulty))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let topic_list = candidate_topics
        .iter()
        .map(|t| format!("- topicId: \"{}\", name: \"{}\"", t.id, t.name))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are a language tutor. Generate {count} connected translation exercises from {native} to {target}.

Target topics: {target_names}
Target difficulties: {difficulty_hint}
Side topics: {side_names}
Native language: {native}
{cefr_hint}
{age_hint}

Use ONLY the following topic IDs when tagging exercises. Do not invent new IDs.
{topic_list}

The {count} sentences should form a short coherent dialogue or mini-story. Keep each sentence natural and focused on the target topics (or general vocabulary if no topics are specified). Adjust the overall complexity to the student's CEFR level if provided.

For each exercise output a JSON object with these fields:
- id: unique string
- targetSentence: sentence in {native} for the student to translate
- expectedTranslation: correct translation in {target}
- targetTopicIds: array of target topic ids from the list above (use empty array if none apply)
- sideTopicIds: array of side topic ids from the list above (use empty array if none apply)
- expectedPatterns: grammar patterns the student should use
- hint: optional short hint

Output a JSON object with a single key \"exercises\" containing an array of the exercise objects.",
        native = profile.native_language,
        target = profile.target_language
    )
}

pub fn build_batch_analysis_prompt(
    profile: &UserProfile,
    pairs: &[(Exercise, String)],
    topics: &[Topic],
) -> String {
    let blocks = pairs
        .iter()
        .enumerate()
        .map(|(i, (exercise, answer))| {
            format!(
                "Exercise {}:\nOriginal ({}): {}\nExpected translation: {}\nStudent translation: {}",
                i + 1,
                profile.target_language,
                exercise.target_sentence,
                exercise.expected_translation,
                answer
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let topic_list = topics
        .iter()
        .map(|t| format!("- topicId: \"{}\", name: \"{}\"", t.id, t.name))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are a strict grammar tutor. The student translated {n} sentence(s). Evaluate each one.

{blocks}

Use ONLY the following topic IDs when tagging errors. Do not invent new IDs.
{topic_list}

Evaluation rules:
- Do NOT penalize or report missing accents, diacritics, punctuation marks (¡, ¿, ., ,, etc.), or capitalization differences.
- Treat "i" and "í", "a" and "á", "e" and "é", etc. as equivalent.
- Focus errors on grammar, vocabulary, word order, and missing or wrong words.
- Keep feedback concise and actionable.
- Each error type must be exactly one of: "critical" | "major" | "minor".
- For each error, include `topicIds` from the list above that the error relates to. If the error involves a concept not covered by the list, also include `newTopics` with `name`, `description`, and `level` (CEFR, e.g. "A1").

New topic rules (CRITICAL):
- Each newTopic must be a SPECIFIC, narrow grammar or vocabulary point that can be practiced through translation exercises.
- AVOID broad, abstract categories such as "Common Spelling Errors", "Grammar Basics", "Vocabulary", "Advanced Topics", "Common Mistakes", or "Fundamentals".
- Good examples: "Relative pronouns: who vs which", "Spelling: experience vs expertise", "Word order in subordinate clauses", "Preterite of irregular verbs: venir".
- Bad examples: "Common Spelling Errors", "Grammar mistakes", "Basic vocabulary".
- The name should be 2-6 words and describe a concrete rule, pattern, or contrast.

Return a JSON object exactly in this shape:
{{
  "sentences": [
    {{
      "sentenceNumber": 1,
      "errors": [{{ "type": "major", "pattern": "...", "explanation": "...", "topicIds": ["..."], "newTopics": [{{ "name": "...", "description": "...", "level": "A1" }}] }}],
      "perSentenceFeedback": [{{ "comment": "..." }}]
    }}
  ],
  "evaluatedTopics": [{{ "topicId": "...", "score": 80.0 }}]
}}

The "errors" array may be empty. When an error has no known curriculum topic, `topicIds` may be empty; use `newTopics` to suggest a new topic. When `topicIds` is empty and no `newTopics` are given, the error will be treated as affecting all topics practiced in that sentence.

CRITICAL: the top-level object MUST contain the key "sentences" with exactly {n} items.
CRITICAL: explanations and comments must be in {native}.
CRITICAL: do not include any markdown code fences."#,
        n = pairs.len(),
        native = profile.native_language
    )
}

pub fn build_topic_review_prompt(profile: &UserProfile, topic: &Topic) -> String {
    format!(
        "You are a language tutor. Prepare a focused review of the topic \"{}\" in {} for a {} speaker.

Topic description: {}

Requirements:
- Explain the core rule in 2-3 short paragraphs.
- Provide 3-5 simple examples in {} with {} translations.
- Avoid introductions, conclusions, and filler text.
- Explain everything in {}.
- CRITICAL: Do NOT mention system instructions, skills, superpowers, tools, the current lesson, or any meta commentary. Only output the topic explanation.

Format your response as well-structured Markdown that the terminal renderer supports:
- Use ## for section headings (e.g. ## Conjugation, ## Examples, ## Usage)
- Use **bold** for emphasis on key terms
- Use `code` for short linguistic examples, forms, or patterns inline
- Use bullet lists (-) for examples and comparisons; do NOT use fenced code blocks (```) or markdown tables
- Keep examples short and use plain ASCII-friendly punctuation where possible; avoid rare Unicode symbols or combining diacritics that may not render in every terminal

Return ONLY the Markdown content. Do not wrap output in code fences.",
        topic.name,
        profile.target_language,
        profile.native_language,
        topic.description,
        profile.target_language,
        profile.native_language,
        profile.native_language
    )
}

pub fn build_curriculum_extension_prompt(
    profile: &UserProfile,
    existing_topics: &[Topic],
    progress: &[ProgressTopic],
    count: usize,
) -> String {
    let existing_lines: Vec<String> = existing_topics
        .iter()
        .map(|t| {
            format!(
                "- {} [{} | {}]: {}",
                t.name,
                t.difficulty,
                t.level.as_deref().unwrap_or("?"),
                t.description
            )
        })
        .collect();
    let weak_lines: Vec<String> = progress
        .iter()
        .filter(|p| p.score < 50.0)
        .map(|p| format!("- {}: score {:.0}", p.topic_id, p.score))
        .collect();
    let cefr = profile.self_assessed_cefr.as_deref().unwrap_or("beginner");
    let age_hint = profile
        .age
        .map(|age| format!("Student age: {age}. Avoid childish topics unless appropriate."))
        .unwrap_or_else(|| "Student age: not specified.".to_string());
    format!(
        "You are expanding a language learning curriculum for {target} for a {native} speaker.\n\
        Goal: general fluency.\n\
        {age_hint}\n\
        Student's current CEFR level: {cefr}.\n\n\
        Existing curriculum topics:\n\
        {existing}\n\n\
        Topics the student is struggling with (score < 50):\n\
        {weak}\n\n\
        Generate exactly {count} new topics that extend or refine the existing curriculum. Consider:\n\
        1. Filling gaps between existing topics (e.g., if 'Preterite: Regular -ar Verbs' exists but 'Preterite: Irregular Verbs' does not, add the missing one).\n\
        2. Reinforcing weak areas if any.\n\
        3. Adding related grammar, vocabulary, usage, or register topics not yet covered.\n\
        4. Progressing toward C2 from the student's current level.\n\n\
        The new topics must NOT duplicate existing topics by name or concept.\n\n\
        Return a JSON object:\n\
        {{ \"topics\": [ {{ \"id\": string, \"name\": string, \"description\": string, \"difficulty\": \"beginner\" | \"intermediate\" | \"advanced\", \"level\": \"A1\" | \"A2\" | \"B1\" | \"B2\" | \"C1\" | \"C2\", \"tags\": string[] }} ] }}\n\n\
        Each topic must include:\n\
        - id: kebab-case string\n\
        - name: short display name (2-6 words)\n\
        - description: 1-2 sentences\n\
        - difficulty: appropriate for the CEFR level\n\
        - level: CEFR level (A1-C2), at or above the student's current level {cefr} unless it fills a clear gap below it\n\
        - tags: relevant grammar/vocabulary tags\n\
        - targetLang: \"{target}\"\n\
        - nativeLang: \"{native}\"\n\
        - version: 1",
        target = profile.target_language,
        native = profile.native_language,
        cefr = cefr,
        age_hint = age_hint,
        existing = existing_lines.join("\n"),
        weak = if weak_lines.is_empty() {
            "(none)".to_string()
        } else {
            weak_lines.join("\n")
        },
        count = count
    )
}

pub fn build_topic_metadata_prompt(topic_id: &str, profile: &UserProfile) -> String {
    format!(
        "Generate a language learning topic for the topic id \"{}\" in {} for a {} speaker.\n\
        The student's current CEFR level is {}. The topic should be appropriate for this level or below if it is a prerequisite topic.\n\
        \n\
        Return a JSON object with:\n\
        - id: \"{}\" (exactly this id)\n\
        - name: short display name (2-5 words)\n\
        - description: 1-2 sentences explaining the topic\n\
        - difficulty: \"beginner\" | \"intermediate\" | \"advanced\"\n\
        - level: CEFR level (\"A1\", \"A2\", \"B1\", \"B2\", \"C1\", \"C2\")\n\
        - tags: string[] (relevant grammar/vocabulary tags)\n\
        - targetLang: \"{}\"\n\
        - nativeLang: \"{}\"\n\
        - version: 1\n\
        \n\
        Respond ONLY with the JSON object. No extra commentary.",
        topic_id,
        profile.target_language,
        profile.native_language,
        profile.self_assessed_cefr.as_deref().unwrap_or("beginner"),
        topic_id,
        profile.target_language,
        profile.native_language
    )
}

pub fn build_curriculum_level_prompt(
    profile: &UserProfile,
    level: &str,
    previous_level: Option<&str>,
    count: usize,
) -> String {
    let difficulty = cefr_to_difficulty(level);
    let previous = previous_level.unwrap_or("beginner");
    let domains = CURRICULUM_DOMAIN_DESCRIPTIONS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| format!("{}. {} — {}", i + 1, name, desc))
        .collect::<Vec<_>>()
        .join("\n");
    let age_hint = profile
        .age
        .map(|age| format!("The student is {age} years old. Avoid school, kindergarten, or other child-specific scenarios unless the age makes them clearly relevant."))
        .unwrap_or_else(|| "The student's age is not specified; keep contexts neutral and broadly applicable.".to_string());
    format!(
        "You are a senior professor of linguistics and language pedagogy. You are designing a focused {target} course for a {native} speaker.\n\
        \n\
        {age_hint}\n\
        \n\
        This course is delivered entirely through translation exercises (sentences and short written texts). Generate ONLY topics that can be practiced by translating from {native} to {target} or analyzing written {target}. Do NOT include listening, speaking, pronunciation drills, or conversation-only topics.\n\
        \n\
        Your current task: produce around {count} focused {target} topics a learner must master to progress from CEFR {previous} to CEFR {level}. Cover each translatable domain listed below with a few concrete, narrow topics. Prefer small, actionable topics that fit 1–2 translation exercises.\n\
        \n\
        All topics in this level must have:\n\
        - difficulty: \"{difficulty}\"\n\
        - level: \"{level}\"\n\
        - tags: include exactly one domain tag from the list below (prefix \"domain:\"), plus relevant grammar/vocabulary tags.\n\
        \n\
        You must cover ALL of the following translatable domains:\n\
        {domains}\n\
        \n\
        Topic format rules:\n\
        - id: unique kebab-case string (lowercase letters, digits, and hyphens only)\n\
        - name: 2-6 words, specific and actionable\n\
        - description: 1-2 sentences\n\
        - difficulty: \"{difficulty}\"\n\
        - level: \"{level}\"\n\
        - tags: [\"domain:<domain-name>\", ...]\n\
        - targetLang: \"{target}\"\n\
        - nativeLang: \"{native}\"\n\
        - version: 1\n\
        \n\
        Return a JSON object:\n\
        {{\n\
          \"version\": 1,\n\
          \"targetLanguage\": \"{target}\",\n\
          \"nativeLanguage\": \"{native}\",\n\
          \"topics\": [ ... ]\n\
        }}\n\
        \n\
        Keep the response concise and valid JSON. Do not include commentary or markdown code fences.",
        target = profile.target_language,
        native = profile.native_language,
        level = level,
        previous = previous,
        difficulty = difficulty,
        domains = domains,
        count = count,
        age_hint = age_hint
    )
}

pub fn build_curriculum_gap_prompt(
    profile: &UserProfile,
    level: &str,
    missing_domains: &[&str],
) -> String {
    let difficulty = cefr_to_difficulty(level);
    format!(
        "You are a senior professor of linguistics and language pedagogy. The {target} curriculum for CEFR level {level} is missing topics in the following domains: {domains}.\n\
        \n\
        Generate exactly the topics needed to cover these domains. Each topic must be translatable in written form (no listening/speaking-only topics). Do not duplicate topics the learner already has at this level.\n\
        \n\
        Return a JSON object:\n\
        {{\n\
          \"version\": 1,\n\
          \"targetLanguage\": \"{target}\",\n\
          \"nativeLanguage\": \"{native}\",\n\
          \"topics\": [\n\
            {{\n\
              \"id\": \"kebab-case id\",\n\
              \"name\": \"2-6 words\",\n\
              \"description\": \"1-2 sentences\",\n\
              \"difficulty\": \"{difficulty}\",\n\
              \"level\": \"{level}\",\n\
              \"tags\": [\"domain:<domain-name>\", ...],\n\
              \"targetLang\": \"{target}\",\n\
              \"nativeLang\": \"{native}\",\n\
              \"version\": 1\n\
            }}\n\
          ]\n\
        }}",
        target = profile.target_language,
        native = profile.native_language,
        level = level,
        difficulty = difficulty,
        domains = missing_domains.join(", ")
    )
}

pub fn build_curriculum_domain_prompt(
    profile: &UserProfile,
    level: &str,
    domain: &str,
    domain_description: &str,
    count: usize,
) -> String {
    let difficulty = cefr_to_difficulty(level);
    format!(
        "You are a senior professor of linguistics and language pedagogy. You are designing {target} topics for CEFR level {level} for a {native} speaker.\n\n\
        Focus ONLY on the following domain. Do not include topics from other domains.\n\
        Domain: {domain} — {domain_description}\n\n\
        This course is delivered entirely through translation exercises (sentences and short written texts). Generate ONLY topics that can be practiced by translating from {native} to {target} or analyzing written {target}. Do NOT include listening, speaking, pronunciation drills, or conversation-only topics.\n\n\
        Generate exactly {count} focused, narrow {target} topics in this domain that a learner must master to progress at CEFR {level}. Each topic should be small enough to practice in 1–2 translation exercises.\n\n\
        All topics must have:\n\
        - difficulty: \"{difficulty}\"\n\
        - level: \"{level}\"\n\
        - tags: include exactly one domain tag \"domain:{domain}\", plus relevant grammar/vocabulary tags.\n\n\
        Topic format rules:\n\
        - id: unique kebab-case string (lowercase letters, digits, and hyphens only)\n\
        - name: 2-6 words, specific and actionable\n\
        - description: 1-2 sentences\n\
        - difficulty: \"{difficulty}\"\n\
        - level: \"{level}\"\n\
        - tags: [\"domain:{domain}\", ...]\n\
        - targetLang: \"{target}\"\n\
        - nativeLang: \"{native}\"\n\
        - version: 1\n\n\
        Return a JSON object:\n\
        {{\n\
          \"version\": 1,\n\
          \"targetLanguage\": \"{target}\",\n\
          \"nativeLanguage\": \"{native}\",\n\
          \"topics\": [ ... ]\n\
        }}\n\n\
        Before returning, double-check that each topic is narrow, has a valid kebab-case id, and belongs to the {domain} domain.",
        target = profile.target_language,
        native = profile.native_language,
        level = level,
        difficulty = difficulty,
        domain = domain,
        domain_description = domain_description,
        count = count,
    )
}

pub fn build_new_topic_metadata_prompt(profile: &UserProfile, new_topic: &NewTopicRef) -> String {
    let cefr = new_topic
        .level
        .as_deref()
        .or(profile.self_assessed_cefr.as_deref())
        .unwrap_or("A1");
    let proposed_level = new_topic.level.as_deref().unwrap_or("A1");
    format!(
        "You are expanding a language learning curriculum for {target} for a {native} speaker.\n\
        The learner's current CEFR level is {cefr}.\n\n\
        Generate a curriculum topic based on this learner error:\n\
        - Proposed name: {name}\n\
        - Proposed description: {description}\n\
        - Proposed CEFR level: {level}\n\n\
        Return a JSON object:\n\
        {{ \"id\": \"kebab-case-id\", \"name\": \"...\", \"description\": \"...\", \"difficulty\": \"beginner\" | \"intermediate\" | \"advanced\", \"level\": \"A1\" | \"A2\" | \"B1\" | \"B2\" | \"C1\" | \"C2\", \"tags\": string[], \"targetLang\": \"{target}\", \"nativeLang\": \"{native}\", \"version\": 1 }}\n\n\
        The id must be unique kebab-case. The name should be 2-6 words. The description should be 1-2 sentences. The difficulty must match the CEFR level.\n\n\
        Respond ONLY with the JSON object. No markdown code fences.",
        target = profile.target_language,
        native = profile.native_language,
        cefr = cefr,
        name = new_topic.name,
        description = new_topic.description,
        level = proposed_level,
    )
}
