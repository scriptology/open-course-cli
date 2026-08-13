//! Applying a finished session's analysis to the database: topic mastery
//! updates, history records, adaptive alerts and learning items.

use std::collections::{HashMap, HashSet};

use chrono::Utc;

use crate::curriculum::Topic;
use crate::forms::Form;
use crate::history::{HistoryTable, SessionSummary};
use crate::learning_items::{
    LearningItem, is_duplicate_name, is_learning_item_name, significant_words,
    text_contains_any_word,
};
use crate::lemmas::Lemma;
use crate::progress::{ProgressData, ProgressTopic};
use open_course_core::curriculum::{CEFR_LEVELS, Curriculum, cefr_to_numeric};
use open_course_core::error::Result;
use open_course_core::session::models::{AnalysisResult, VocabularyUse};
use open_course_core::session::scoring::{
    adaptive_alpha, average, clamp_score, ema_update, topic_exercise_scores,
};
use open_course_core::session::{
    LOW_SESSION_SCORE_THRESHOLD, MASTERY_THRESHOLD, MentorSession, unique_topic_ids,
};
use open_course_core::vocabulary::{
    STATUS_NEW, derive_status, find_form_fuzzy, find_lemma, normalize_feats_key,
    should_replace_cefr, vocabulary_session_score,
};

pub async fn apply_analysis(
    analysis: &AnalysisResult,
    session: &MentorSession,
    progress_data: &mut ProgressData,
    history_table: &HistoryTable,
) -> Result<HashMap<String, f64>> {
    let target_ids = unique_topic_ids(
        session
            .exercises
            .iter()
            .flat_map(|e| e.target_topic_ids.iter().cloned()),
    );
    let side_ids = unique_topic_ids(
        session
            .exercises
            .iter()
            .flat_map(|e| e.side_topic_ids.iter().cloned()),
    );
    let session_topic_ids = unique_topic_ids(
        target_ids
            .iter()
            .cloned()
            .chain(side_ids.iter().cloned())
            .chain(
                analysis
                    .sentences
                    .iter()
                    .flat_map(|s| s.errors.iter().flat_map(|e| e.topic_ids.iter().cloned())),
            ),
    );

    let exercise_scores_by_topic = topic_exercise_scores(session, analysis);

    let mut final_scores = HashMap::new();
    let now = Utc::now().to_rfc3339();

    for topic_id in &session_topic_ids {
        let scores = exercise_scores_by_topic
            .get(topic_id)
            .cloned()
            .unwrap_or_default();
        let existing = progress_data
            .topics
            .iter()
            .find(|t| &t.topic_id == topic_id);
        let base_mastery = existing.map(|t| t.mastery).unwrap_or(0.0);
        let mut mastery = base_mastery;
        let mut practice_count = existing.map(|t| t.practice_count).unwrap_or(0);
        for exercise_score in scores {
            let alpha = adaptive_alpha(mastery);
            mastery = mastery * (1.0 - alpha) + exercise_score * alpha;
            practice_count += 1;
        }
        mastery = clamp_score(mastery.round());

        final_scores.insert(topic_id.clone(), mastery);

        let updated = ProgressTopic {
            topic_id: topic_id.clone(),
            score: mastery,
            mastery,
            difficulty_estimate: existing.map(|t| t.difficulty_estimate).unwrap_or(0.0),
            practice_count,
            last_practiced: Some(now.clone()),
            ..Default::default()
        };

        if let Some(pos) = progress_data
            .topics
            .iter()
            .position(|t| &t.topic_id == topic_id)
        {
            progress_data.topics[pos] = updated;
        } else {
            progress_data.topics.push(updated);
        }
    }

    let target_scores: Vec<f64> = target_ids
        .iter()
        .map(|id| *final_scores.get(id).unwrap_or(&0.0))
        .collect();
    let avg_target_score = average(&target_scores);

    let summary = SessionSummary {
        id: session.id.clone(),
        date: now,
        target_topic_ids: target_ids,
        side_topic_ids: side_ids,
        new_topic_ids: analysis.new_topics.iter().map(|t| t.id.clone()).collect(),
        avg_target_score,
        target_delta: 0.0,
        ..Default::default()
    };

    history_table.append(&summary).await?;

    progress_data.session_count += 1;

    let mut alerts = Vec::new();
    if avg_target_score < LOW_SESSION_SCORE_THRESHOLD {
        alerts.push("low_session_score".to_string());
    }
    if analysis.sentences.iter().any(|s| !s.errors.is_empty()) {
        alerts.push("review_session_errors".to_string());
    }
    if progress_data
        .topics
        .iter()
        .any(|t| t.score < MASTERY_THRESHOLD)
    {
        alerts.push("focus_on_weak_topics".to_string());
    }
    progress_data.adaptive_alerts.extend(alerts);
    progress_data.adaptive_alerts.sort();
    progress_data.adaptive_alerts.dedup();

    Ok(final_scores)
}

/// Returns the final topic masteries plus the ids of every lemma and form
/// the session touched (created, practiced, or CEFR-updated), so the caller
/// can emit outbox entries for exactly the rows that changed.
///
/// Forced lemmas earn practice credit only through student-side evidence:
/// `_forced_lemma_ids` is kept for call-site symmetry but a forced lemma
/// without an observed use is left completely untouched.
pub async fn apply_analysis_to_db(
    analysis: &mut AnalysisResult,
    session: &MentorSession,
    forced_learning_item_ids: &[String],
    _forced_lemma_ids: &[String],
    db: &crate::Database,
) -> Result<(HashMap<String, f64>, Vec<String>, Vec<String>)> {
    let mut progress = db.progress().read_all().await?;

    let mut learning_items: HashMap<String, LearningItem> = db
        .learning_items()
        .read_all()
        .await?
        .into_iter()
        .map(|li| (li.id.clone(), li))
        .collect();

    // (id, name) of every known learning item and curriculum topic, sorted
    // for deterministic fuzzy-dedup matching below.
    let mut known_items: Vec<(String, String)> = learning_items
        .values()
        .map(|li| (li.id.clone(), li.name.clone()))
        .collect();
    known_items.sort();
    let existing_curriculum = db.curriculum().read_all().await?;
    let mut known_topics: Vec<(String, String)> = existing_curriculum
        .topics
        .iter()
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect();
    known_topics.sort();

    // Items that get practice credit this session: the forced ones plus any
    // existing item a new entry was deduplicated into.
    let mut practiced_item_ids: Vec<String> = forced_learning_item_ids.to_vec();

    // Word-specific items (e.g. "Adjective: Caro vs Rico") are stored as
    // learning items for later review, not as curriculum topics. Entries that
    // already exist keep their accumulated score; fuzzy duplicates are merged
    // into the existing item instead of being created.
    for item in &analysis.new_learning_items {
        insert_learning_item(
            &mut learning_items,
            &mut known_items,
            &mut practiced_item_ids,
            item.clone(),
        );
    }

    for topic in &analysis.new_topics {
        // Safety net: word-specific names must not become curriculum topics.
        if is_learning_item_name(&topic.name) {
            let item = LearningItem::from_topic(topic);
            insert_learning_item(
                &mut learning_items,
                &mut known_items,
                &mut practiced_item_ids,
                item,
            );
            continue;
        }
        let topic_id = match find_duplicate_topic(&known_topics, topic) {
            Some(existing_id) => existing_id,
            None => {
                db.curriculum().upsert(topic).await?;
                match known_topics.iter_mut().find(|(id, _)| id == &topic.id) {
                    Some(entry) => entry.1 = topic.name.clone(),
                    None => known_topics.push((topic.id.clone(), topic.name.clone())),
                }
                topic.id.clone()
            }
        };
        if !progress.topics.iter().any(|p| p.topic_id == topic_id) {
            progress.topics.push(ProgressTopic::initial(topic_id, 0.0));
        }
    }

    let now = Utc::now().to_rfc3339();
    for id in &practiced_item_ids {
        if let Some(item) = learning_items.get_mut(id) {
            // Every practiced item gets credit: 0 when a session error is
            // associated with it, 100 otherwise. Practiced means the item was
            // forced into the exercise prompt or matched by a reported error,
            // so no occurrence detection in the session text is needed — the
            // item name's language often differs from the exercise texts.
            let session_score = if item_has_error(item, analysis) {
                0.0
            } else {
                100.0
            };
            item.score = ema_update(item.score, session_score);
            item.last_practiced = Some(now.clone());
            item.practice_count += 1;
        }
    }

    for item in learning_items.values() {
        db.learning_items().upsert(item).await?;
    }

    // Vocabulary: lemmas and forms mentioned in `used_vocabulary`. Words
    // from the target sentence (side "target") are only registered as NEW;
    // words from the student's translation (side "student") carry per-use
    // assessments and produce scoring evidence. Both sides ensure the
    // lemma/form exists — student-side evidence needs an entity to attach
    // to even when the word never appeared in the target sentence.
    let mut lemmas: Vec<Lemma> = db.lemmas().read_all().await?;
    let mut forms: Vec<Form> = db.forms().read_all().await?;
    // Worst-case evidence per entity: the lowest session score across all
    // observed uses, plus whether any use had an error.
    let mut lemma_evidence: HashMap<String, (f64, bool)> = HashMap::new();
    let mut form_evidence: HashMap<String, (f64, bool)> = HashMap::new();
    let mut created_lemma_ids: Vec<String> = Vec::new();
    let mut created_form_ids: Vec<String> = Vec::new();
    // Existing forms whose feats were enriched by a richer incoming variant
    // through fuzzy-merge; they must be persisted even without evidence.
    let mut enriched_form_ids: HashSet<String> = HashSet::new();
    // Lemmas whose CEFR level was upgraded by a higher-ranked source without
    // any scoring evidence; they still need to be persisted.
    let mut cefr_updated_lemma_ids: HashSet<String> = HashSet::new();

    for sentence in &analysis.sentences {
        for vocabulary_use in &sentence.used_vocabulary {
            if vocabulary_use.lemma.is_empty() {
                continue;
            }
            let cefr = cefr_candidate(
                session,
                &existing_curriculum,
                sentence.sentence_number,
                vocabulary_use,
            );
            let lemma_pos = match find_lemma(
                &lemmas,
                &existing_curriculum.target_language,
                &vocabulary_use.lemma,
                &vocabulary_use.pos,
            ) {
                Some(pos) => {
                    // Reappearance of a known lemma: a strictly higher-ranked
                    // source replaces the stored level (topic overrides an
                    // LLM guess, never the other way round).
                    if let Some((level, source)) = &cefr
                        && should_replace_cefr(lemmas[pos].cefr_source.as_deref(), source)
                    {
                        lemmas[pos].cefr_level = Some(level.clone());
                        lemmas[pos].cefr_source = Some(source.to_string());
                        cefr_updated_lemma_ids.insert(lemmas[pos].id.clone());
                    }
                    pos
                }
                None => {
                    let (cefr_level, cefr_source) = match &cefr {
                        Some((level, source)) => {
                            (Some(level.clone()), Some(source.to_string()))
                        }
                        None => (None, None),
                    };
                    let mut lemma = Lemma {
                        id: Lemma::slug_id(
                            &vocabulary_use.lemma,
                            &existing_curriculum.target_language,
                        ),
                        lemma: vocabulary_use.lemma.clone(),
                        pos: vocabulary_use.pos.clone(),
                        target_lang: existing_curriculum.target_language.clone(),
                        native_lang: existing_curriculum.native_language.clone(),
                        status: STATUS_NEW,
                        cefr_level,
                        cefr_source,
                        ..Default::default()
                    };
                    // The slug id is taken by a different (lemma, pos):
                    // disambiguate with a POS suffix.
                    if lemmas.iter().any(|l| l.id == lemma.id) {
                        lemma.id =
                            format!("{}-{}", lemma.id, vocabulary_use.pos.to_lowercase());
                    }
                    created_lemma_ids.push(lemma.id.clone());
                    lemmas.push(lemma);
                    lemmas.len() - 1
                }
            };
            let lemma_id = lemmas[lemma_pos].id.clone();

            if vocabulary_use.surface.is_empty() {
                continue;
            }
            let feats_key = normalize_feats_key(&vocabulary_use.feats);
            let form_pos =
                match find_form_fuzzy(&forms, &lemma_id, &vocabulary_use.surface, &feats_key) {
                    Some(pos) => {
                        // Fuzzy hit: the incoming use merges into the existing
                        // form instead of creating a near-duplicate. When the
                        // incoming feats set is richer (more segments) than
                        // the stored one, upgrade feats/feats_key to the
                        // fuller description and persist the form.
                        if feats_segment_count(&feats_key)
                            > feats_segment_count(&forms[pos].feats_key)
                        {
                            forms[pos].feats = vocabulary_use.feats.clone();
                            forms[pos].feats_key = feats_key.clone();
                            enriched_form_ids.insert(forms[pos].id.clone());
                        }
                        Some(pos)
                    }
                    None => {
                        let base_id = Form::id(&lemma_id, &vocabulary_use.surface);
                        let mut id = base_id.clone();
                        // feats_key collisions on the same surface get numeric
                        // suffixes (same pattern as topic_id_from_name).
                        let mut suffix = 0;
                        while forms.iter().any(|f| f.id == id) {
                            suffix += 1;
                            id = format!("{base_id}-{suffix}");
                        }
                        let form = Form {
                            id,
                            lemma_id: lemma_id.clone(),
                            surface: vocabulary_use.surface.clone(),
                            feats: vocabulary_use.feats.clone(),
                            feats_key,
                            status: STATUS_NEW,
                            ..Default::default()
                        };
                        created_form_ids.push(form.id.clone());
                        forms.push(form);
                        Some(forms.len() - 1)
                    }
                };

            if vocabulary_use.side != "student" {
                continue;
            }
            // A missing assessment (None) is treated conservatively as OK:
            // the LLM omits the flags when nothing is wrong, so an absent
            // flag must not penalize the word.
            let session_score = vocabulary_session_score(
                vocabulary_use.spelling_ok.unwrap_or(true),
                vocabulary_use.usage_ok.unwrap_or(true),
            );
            let had_error = session_score < 100.0;
            lemma_evidence
                .entry(lemma_id.clone())
                .and_modify(|(score, err)| {
                    *score = score.min(session_score);
                    *err = *err || had_error;
                })
                .or_insert((session_score, had_error));
            if let Some(pos) = form_pos {
                let form_id = forms[pos].id.clone();
                form_evidence
                    .entry(form_id)
                    .and_modify(|(score, err)| {
                        *score = score.min(session_score);
                        *err = *err || had_error;
                    })
                    .or_insert((session_score, had_error));
            }
        }
    }

    // Scored lemmas: only those with student-side evidence. A forced lemma
    // the student never used earns no credit — its mastery, counters,
    // last_seen and status stay untouched and it is not marked touched
    // (neutral non-use instead of the former soft credit with a perfect
    // score). Forms likewise score only through evidence.
    let mut touched_lemma_ids: HashSet<String> = created_lemma_ids.iter().cloned().collect();
    touched_lemma_ids.extend(cefr_updated_lemma_ids);
    let mut touched_form_ids: HashSet<String> = created_form_ids.iter().cloned().collect();
    touched_form_ids.extend(enriched_form_ids);

    for (id, (session_score, had_error)) in &lemma_evidence {
        if let Some(lemma) = lemmas.iter_mut().find(|l| &l.id == id) {
            lemma.mastery = ema_update(lemma.mastery, *session_score);
            lemma.practice_count += 1;
            if *had_error {
                lemma.incorrect_uses += 1;
            } else {
                lemma.correct_uses += 1;
            }
            lemma.last_seen = Some(now.clone());
            lemma.status = derive_status(lemma.mastery, *had_error);
            touched_lemma_ids.insert(id.clone());
        }
    }

    for (id, (session_score, had_error)) in &form_evidence {
        if let Some(form) = forms.iter_mut().find(|f| &f.id == id) {
            form.mastery = ema_update(form.mastery, *session_score);
            if *had_error {
                form.incorrect += 1;
            } else {
                form.correct += 1;
            }
            form.last_seen = Some(now.clone());
            form.status = derive_status(form.mastery, *had_error);
            touched_form_ids.insert(id.clone());
        }
    }

    // Report the created entities (post-scoring state) so callers can emit
    // outbox entries and session summaries.
    analysis.new_lemmas = created_lemma_ids
        .iter()
        .filter_map(|id| lemmas.iter().find(|l| &l.id == id).cloned())
        .collect();
    analysis.new_forms = created_form_ids
        .iter()
        .filter_map(|id| forms.iter().find(|f| &f.id == id).cloned())
        .collect();

    for lemma in lemmas.iter().filter(|l| touched_lemma_ids.contains(&l.id)) {
        db.lemmas().upsert(lemma).await?;
    }
    for form in forms.iter().filter(|f| touched_form_ids.contains(&f.id)) {
        db.forms().upsert(form).await?;
    }

    let history = db.history();
    let scores = apply_analysis(analysis, session, &mut progress, &history).await?;
    db.progress().write_all(&progress).await?;
    let mut touched_lemmas: Vec<String> = touched_lemma_ids.into_iter().collect();
    touched_lemmas.sort();
    let mut touched_forms: Vec<String> = touched_form_ids.into_iter().collect();
    touched_forms.sort();
    Ok((scores, touched_lemmas, touched_forms))
}

/// Number of `key=value` segments in a normalized feats key — used to decide
/// which of two fuzzy-merged variants carries the richer description.
fn feats_segment_count(feats_key: &str) -> usize {
    feats_key.split('|').filter(|s| !s.is_empty()).count()
}

/// CEFR candidate for a vocabulary use: the minimum valid level among the
/// curriculum topics targeted by the exercise the sentence belongs to
/// (source "topic", matching the early-textbook-order guidance given to the
/// LLM), falling back to the LLM's per-use estimate (source "llm"). Levels
/// outside `CEFR_LEVELS` are dropped as garbage.
fn cefr_candidate(
    session: &MentorSession,
    curriculum: &Curriculum,
    sentence_number: i32,
    vocabulary_use: &VocabularyUse,
) -> Option<(String, &'static str)> {
    // sentence_number is 1-based and matches the exercise order (validated
    // at parse time).
    if sentence_number >= 1
        && let Some(exercise) = session.exercises.get((sentence_number - 1) as usize)
    {
        let topic_level = exercise
            .target_topic_ids
            .iter()
            .filter_map(|id| curriculum.topics.iter().find(|t| &t.id == id))
            .filter_map(|t| t.level.as_deref())
            .filter(|level| CEFR_LEVELS.contains(level))
            .min_by_key(|level| cefr_to_numeric(level).unwrap_or(i32::MAX));
        if let Some(level) = topic_level {
            return Some((level.to_string(), "topic"));
        }
    }
    vocabulary_use
        .cefr_level
        .as_deref()
        .filter(|level| CEFR_LEVELS.contains(level))
        .map(|level| (level.to_string(), "llm"))
}

/// Inserts a new learning item, merging fuzzy name duplicates into the
/// existing entry: the duplicate is not created and the existing item is
/// scheduled for practice credit this session (appended to `practiced_ids`).
fn insert_learning_item(
    learning_items: &mut HashMap<String, LearningItem>,
    known_items: &mut Vec<(String, String)>,
    practiced_ids: &mut Vec<String>,
    item: LearningItem,
) {
    if learning_items.contains_key(&item.id) {
        return;
    }
    let names: Vec<String> = known_items.iter().map(|(_, name)| name.clone()).collect();
    if let Some(pos) = is_duplicate_name(&names, &item.name) {
        let existing_id = known_items[pos].0.clone();
        if !practiced_ids.contains(&existing_id) {
            practiced_ids.push(existing_id);
        }
        return;
    }
    known_items.push((item.id.clone(), item.name.clone()));
    learning_items.insert(item.id.clone(), item);
}

/// Returns the id of an existing curriculum topic whose name is a fuzzy
/// duplicate of `topic`'s name, or None when the topic is genuinely new.
/// A matching id is an update of the same topic, not a duplicate.
fn find_duplicate_topic(known_topics: &[(String, String)], topic: &Topic) -> Option<String> {
    if known_topics.iter().any(|(id, _)| id == &topic.id) {
        return None;
    }
    let names: Vec<String> = known_topics.iter().map(|(_, name)| name.clone()).collect();
    is_duplicate_name(&names, &topic.name).map(|pos| known_topics[pos].0.clone())
}

/// Whether any session error is associated with the learning item: an
/// error's new topic is a fuzzy name duplicate of the item, or the error's
/// pattern/explanation mentions a significant word of the item name
/// (falling back to the description, then to the legacy full-name substring
/// match when neither yields any words).
fn item_has_error(item: &LearningItem, analysis: &AnalysisResult) -> bool {
    let mut key_words = significant_words(&item.name);
    if key_words.is_empty() {
        key_words = significant_words(&item.description);
    }

    for sentence in &analysis.sentences {
        for error in &sentence.errors {
            if key_words.is_empty() {
                // No usable words at all: legacy full-name matching.
                let name = item.name.to_lowercase();
                if error.pattern.to_lowercase().contains(&name)
                    || error.explanation.to_lowercase().contains(&name)
                    || error
                        .new_topics
                        .iter()
                        .any(|nt| nt.name.to_lowercase().contains(&name))
                {
                    return true;
                }
                continue;
            }
            if text_contains_any_word(&error.pattern, &key_words)
                || text_contains_any_word(&error.explanation, &key_words)
                || error.new_topics.iter().any(|nt| {
                    text_contains_any_word(&nt.name, &key_words)
                        || is_duplicate_name(std::slice::from_ref(&item.name), &nt.name).is_some()
                })
            {
                return true;
            }
        }
    }
    false
}
