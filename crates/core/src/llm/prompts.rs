use crate::curriculum::{
    CURRICULUM_DOMAIN_DESCRIPTIONS, Topic, cefr_to_difficulty, cefr_to_numeric, difficulty_to_cefr,
};
use crate::learning_items::LearningItem;
use crate::profile::UserProfile;
use crate::progress::ProgressTopic;
use crate::session::{Exercise, NewTopicRef};
use crate::vocabulary::Lemma;

/// Per-CEFR-level sentence shape limits for generated exercises, so that
/// "coherent mini-story" does not turn into long multi-clause sentences.
fn sentence_shape_guidance(level: &str) -> &'static str {
    match level.to_uppercase().as_str() {
        "A1" => {
            "3-7 words per sentence, a single clause, present tense only, very common vocabulary."
        }
        "A2" => {
            "6-12 words per sentence, one clause preferred, at most one simple subordinate clause (because/when/that), common everyday vocabulary."
        }
        "B1" => "8-15 words per sentence, up to two clauses, common connectors.",
        "B2" => {
            "up to about 20 words per sentence, subordinate clauses and some idiomatic vocabulary allowed."
        }
        _ => "no length limit, natural sophisticated prose.",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_exercise_prompt(
    profile: &UserProfile,
    target_topics: &[Topic],
    side_topics: &[Topic],
    candidate_topics: &[Topic],
    forced_learning_items: &[LearningItem],
    forced_vocabulary: &[Lemma],
    count: u32,
    recent_success_rate: f64,
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

    let topic_level = |t: &Topic| -> Option<String> {
        t.level
            .as_deref()
            .filter(|l| cefr_to_numeric(l).is_some())
            .map(|l| l.to_uppercase())
            .or_else(|| difficulty_to_cefr(&t.difficulty))
    };
    // The exercise complexity is anchored to the target topics' CEFR level
    // (the highest one when a session mixes topics), not to the student's
    // self-assessed level.
    let effective_level = target_topics
        .iter()
        .filter_map(topic_level)
        .max_by_key(|l| cefr_to_numeric(l).unwrap_or(0));

    let cefr_hint = profile
        .self_assessed_cefr
        .as_ref()
        .map(|c| format!("Proficiency level (self-assessed): {c}"))
        .unwrap_or_default();

    let age = profile.age.unwrap_or(18);
    let age_hint = format!(
        "Student age: {age}. Use contexts and examples that fit the life experience of a typical {age}-year-old."
    );

    let complexity_anchor = effective_level
        .as_deref()
        .unwrap_or("the student's CEFR level");
    let recent_rate_pct = (recent_success_rate * 100.0).round() as i32;
    let adaptive_hint = format!(
        "Recent session success rate: {recent_rate_pct}%. Target success rate: 80%. \
         If recent rate is below 75%, make sentences slightly easier. \
         If recent rate is above 85%, make sentences slightly more challenging. \
         In both cases stay within the sentence-shape limits for {complexity_anchor}."
    );

    let complexity_hint = match &effective_level {
        Some(level) => format!(
            "Exercise complexity level: {level} (from the target topics). \
             Sentence complexity MUST match CEFR level {level}. This overrides the student's \
             self-assessed level: the self-assessed level describes the student overall, but \
             these exercises practice {level} material.\n\
             Sentence shape for {level}: {}",
            sentence_shape_guidance(level)
        ),
        None => {
            "Adjust the overall complexity to the student's CEFR level if provided.".to_string()
        }
    };

    let difficulty_hint = if target_topics.is_empty() {
        "general".to_string()
    } else {
        target_topics
            .iter()
            .map(|t| {
                let level = topic_level(t).unwrap_or_else(|| "unknown".to_string());
                format!("{} (difficulty: {}, CEFR: {})", t.name, t.difficulty, level)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    let topic_list = candidate_topics
        .iter()
        .map(|t| format!("- topicId: \"{}\", name: \"{}\"", t.id, t.name))
        .collect::<Vec<_>>()
        .join("\n");

    let learning_items_hint = if forced_learning_items.is_empty() {
        String::new()
    } else {
        let items = forced_learning_items
            .iter()
            .map(|li| format!("- {} ({})", li.name, li.description))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\nThe following learning items need extra practice. Naturally include EACH of them in at least one exercise, distributing them across different exercises, without distorting the target topics:\n{items}\n"
        )
    };

    let vocabulary_hint = if forced_vocabulary.is_empty() {
        String::new()
    } else {
        let words = forced_vocabulary
            .iter()
            .map(|l| {
                if l.translation.is_empty() {
                    format!("- {}", l.lemma)
                } else {
                    format!("- {} ({})", l.lemma, l.translation)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\nThe following words need extra practice. Include each of these words if it fits naturally, distributing them across different exercises and without distorting the target topics; skip a word rather than distort the sentence:\n{words}\n"
        )
    };

    format!(
        "You are a language tutor. Generate {count} connected translation exercises from {native} to {target}.

Target topics: {target_names}
Target difficulties: {difficulty_hint}
Side topics: {side_names}
Native language: {native}
{cefr_hint}
{age_hint}
{adaptive_hint}
{complexity_hint}

Use ONLY the following topic IDs when tagging exercises. Do not invent new IDs.
{topic_list}{learning_items_hint}{vocabulary_hint}

The {count} sentences should form a short coherent dialogue or mini-story while respecting the sentence-shape limits stated above. Keep each sentence natural and focused on the target topics (or general vocabulary if no topics are specified).

For each exercise output a JSON object with these fields:
- id: unique string
- targetSentence: sentence in {native} for the student to translate
- expectedTranslation: one natural correct translation in {target}
- acceptableTranslations: array of 1–3 additional valid translations in {target} that are semantically equivalent but may use different wording, synonyms, or word order. Include only genuinely equivalent variants.
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
                profile.native_language,
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
        r#"You are a strict grammar tutor. The student is a {native} speaker learning {target}. The student translated {n} sentence(s). Evaluate each one for semantic equivalence and correctness, not for matching a single wording.

{blocks}

Use ONLY the following topic IDs when tagging errors. Do not invent new IDs.
{topic_list}

Evaluation rules:
- Do NOT penalize or report missing accents, diacritics, punctuation marks (¡, ¿, ., ,, etc.), or capitalization differences.
- Treat "i" and "í", "a" and "á", "e" and "é", etc. as equivalent.
- Accept synonyms, alternative word order, and natural paraphrases as correct or acceptable.
- Report an error ONLY when the student's translation is semantically different, grammatically wrong, or misses/adds a meaning-bearing word.
- Keep feedback concise and actionable.
- Each error type must be exactly one of: "critical" | "major" | "minor" | "spelling".
- Use type "spelling" ONLY for simple typos/misspellings where the intended word is obvious (e.g. "Everty" → "Every"). Spelling errors affect the score but must NOT produce newTopics.
- For each error, include `topicIds` from the list above that the error relates to. If the error involves a concept not covered by the list, also include `newTopics` with `name`, `description`, and `level` (CEFR, e.g. "A1"). Do NOT include `newTopics` for spelling errors.

Semantic verdict rules:
- "correct" — the translation is fully equivalent and natural.
- "acceptable" — the meaning is preserved but there is a minor stylistic or less natural choice; include a minor or spelling note if relevant.
- "needsCorrection" — the meaning is wrong or grammar is significantly off; list errors.

New topic rules (CRITICAL):
- Each newTopic must be a generalizable, reusable grammar or usage pattern that can be practiced through many translation exercises (e.g. an agreement rule, a word-order pattern, a verb conjugation class).
- AVOID broad, abstract categories such as "Common Spelling Errors", "Grammar Basics", "Vocabulary", "Advanced Topics", "Common Mistakes", or "Fundamentals".
- AVOID topics tied to a single word or a single word pair. A confusion between specific words (e.g. "Adjective: Caro vs Rico") is NOT a curriculum topic: still report it in newTopics with the word-level name, and the app will store it as a vocabulary review item instead of a topic.
- Good topic examples: "Adjective gender agreement", "Word order in subordinate clauses", "Preterite of irregular verbs: venir", "Ser vs estar with adjectives".
- Bad topic examples: "Common Spelling Errors", "Grammar mistakes", "Basic vocabulary", "Adjective: Caro vs Rico".
- The name should be 2-6 words and describe a concrete rule or pattern.
- Do NOT create newTopics for spelling-only errors.
- Do NOT report newTopics for errors fully explained by a single word form (a wrong inflection, agreement, or choice of one word) — these are already captured through usedVocabulary with spellingOk/usageOk false. Report newTopics only for constructions, multi-word patterns, word pairs, or false friends.
- Every newTopic must be a {target} grammar or usage topic. NEVER create newTopics about any other language.
- If the student answered in the wrong language (e.g. a language other than {target}), mark the affected words as errors, give the correct {target} translation in the explanation, and do NOT create newTopics for that other language.

Vocabulary extraction rules (usedVocabulary):
- For each sentence, list the CONTENT words (NOUN, VERB, ADJ, ADV, PROPN only) in `usedVocabulary`. Skip function words (articles, prepositions, pronouns, auxiliaries, conjunctions).
- Words from the expected translation: side "target", no spellingOk/usageOk fields.
- Words from the student's translation: side "student", WITH spellingOk and usageOk.
- spellingOk is false ONLY for a real misspelling; missing accents, diacritics, punctuation, or capitalization do NOT make spellingOk false.
- usageOk is false when the word is misused: wrong word choice, wrong inflected form, or broken agreement.
- expectedForm is true when the surface matches a form used in the expected (or an acceptable) translation; target-side entries always have expectedForm true.
- `lemma` is the dictionary headword, `pos` is the Universal Dependencies POS tag, `feats` are UD morphological features strictly in "Attr=Val|Attr=Val" format (e.g. "Mood=Ind|Number=Sing|Person=3|Tense=Pres"). Include the full standard UD feature set for the language: always include VerbForm for verbs (e.g. VerbForm=Fin), plus Mood/Tense/Person/Number for finite forms and Gender/Number for nominals, whenever they apply. Use an empty string when no features apply.
- `cefrLevel`: Estimate approximate CEFR level (A1–C2) for the lemma for a typical adult general learner. Prefer high-frequency / early textbook order. Ignore rare or specialized senses.

Return a JSON object exactly in this shape:
{{
  "sentences": [
    {{
      "sentenceNumber": 1,
      "studentTranslation": "...",
      "expectedTranslation": "...",
      "acceptableTranslations": ["...", "..."],
      "semanticVerdict": "correct",
      "errors": [{{ "type": "major", "pattern": "...", "explanation": "...", "topicIds": ["..."], "newTopics": [{{ "name": "...", "description": "...", "level": "A1" }}] }}],
      "perSentenceFeedback": [{{ "comment": "..." }}],
      "usedVocabulary": [
        {{ "surface": "...", "lemma": "...", "pos": "NOUN", "feats": "Gender=Fem|Number=Sing", "side": "target", "expectedForm": true, "cefrLevel": "A2" }},
        {{ "surface": "...", "lemma": "...", "pos": "VERB", "feats": "Mood=Ind|Number=Sing|Person=3|Tense=Pres", "side": "student", "spellingOk": true, "usageOk": true, "expectedForm": true, "cefrLevel": "A1" }}
      ]
    }}
  ],
  "evaluatedTopics": [{{ "topicId": "...", "score": 80.0 }}]
}}

The "errors" and "usedVocabulary" arrays may be empty. When an error has no known curriculum topic, `topicIds` may be empty; use `newTopics` to suggest a new topic. When `topicIds` is empty and no `newTopics` are given, the error will be treated as affecting all topics practiced in that sentence.

CRITICAL: the top-level object MUST contain the key "sentences" with exactly {n} items.
CRITICAL: explanations and comments must be in {native}.
CRITICAL: do not include any markdown code fences."#,
        n = pairs.len(),
        native = profile.native_language,
        target = profile.target_language
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
        - version: 1\n\
        \n\
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.",
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
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.\n\
        \n\
        Respond ONLY with the JSON object. No extra commentary.",
        topic_id,
        profile.target_language,
        profile.native_language,
        profile.self_assessed_cefr.as_deref().unwrap_or("beginner"),
        topic_id,
        profile.target_language,
        profile.native_language,
        native = profile.native_language,
        target = profile.target_language
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
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.\n\
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
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.\n\
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
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.\n\n\
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
        CRITICAL: write each topic's \"name\" and \"description\" in {native} (the student's native language). Only linguistic examples may be in {target}.\n\n\
        Respond ONLY with the JSON object. No markdown code fences.",
        target = profile.target_language,
        native = profile.native_language,
        cefr = cefr,
        name = new_topic.name,
        description = new_topic.description,
        level = proposed_level,
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> UserProfile {
        UserProfile {
            native_language: "Russian".to_string(),
            target_language: "Spanish".to_string(),
            age: Some(30),
            self_assessed_cefr: Some("A2".to_string()),
        }
    }

    #[test]
    fn exercise_prompt_includes_forced_vocabulary_block() {
        let lemmas = vec![
            Lemma {
                lemma: "comer".to_string(),
                translation: "есть".to_string(),
                ..Default::default()
            },
            Lemma {
                lemma: "pequeño".to_string(),
                ..Default::default()
            },
        ];
        let prompt = build_exercise_prompt(&profile(), &[], &[], &[], &[], &lemmas, 3, 0.8);
        assert!(prompt.contains("The following words need extra practice"));
        assert!(prompt.contains("- comer (есть)"));
        assert!(prompt.contains("- pequeño\n"));
        // Forced words are included only when they fit naturally.
        assert!(prompt.contains("if it fits naturally"));
        assert!(prompt.contains("skip a word rather than distort the sentence"));
    }

    #[test]
    fn exercise_prompt_omits_forced_vocabulary_block_when_empty() {
        let prompt = build_exercise_prompt(&profile(), &[], &[], &[], &[], &[], 3, 0.8);
        assert!(!prompt.contains("need extra practice"));
    }

    #[test]
    fn batch_analysis_prompt_describes_used_vocabulary() {
        let exercise = Exercise {
            id: "ex1".to_string(),
            target_sentence: "Я ем".to_string(),
            expected_translation: "Como".to_string(),
            acceptable_translations: vec![],
            target_topic_ids: vec![],
            side_topic_ids: vec![],
            expected_patterns: vec![],
            hint: None,
        };
        let pairs = vec![(exercise, "Como".to_string())];
        let prompt = build_batch_analysis_prompt(&profile(), &pairs, &[]);
        assert!(prompt.contains("\"usedVocabulary\""));
        assert!(prompt.contains("side \"target\""));
        assert!(prompt.contains("side \"student\""));
        assert!(prompt.contains("spellingOk"));
        assert!(prompt.contains("Attr=Val"));
        assert!(prompt.contains("NOUN, VERB, ADJ, ADV, PROPN"));
        assert!(prompt.contains("\"cefrLevel\""));
        assert!(prompt.contains("Estimate approximate CEFR level (A1–C2) for the lemma for a typical adult general learner. Prefer high-frequency / early textbook order. Ignore rare or specialized senses."));
        // Full UD feature set is requested, including VerbForm for verbs.
        assert!(prompt.contains("full standard UD feature set"));
        assert!(prompt.contains("VerbForm=Fin"));
        // Single-word-form errors must not produce newTopics.
        assert!(prompt.contains("errors fully explained by a single word form"));
    }
}
