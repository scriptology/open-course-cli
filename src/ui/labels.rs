use crate::core::language::normalize_language_code;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReportLabels {
    pub translate: &'static str,
    pub loading_exercises: &'static str,
    pub loading_analysis: &'static str,
    pub loading_curriculum: &'static str,
    pub no_exercise: &'static str,
    pub select_topic: &'static str,
    pub choose_topic: &'static str,
    pub your_answer: &'static str,
    pub submit: &'static str,
    pub back: &'static str,
    pub cancel: &'static str,
    pub navigate: &'static str,
    pub per_exercise_results: &'static str,
    pub topic_scores: &'static str,
    pub no_errors: &'static str,
    pub session_report: &'static str,
    pub your_translation: &'static str,
    pub correct_answer: &'static str,
    pub feedback: &'static str,
    pub score: &'static str,
    pub weak_topics: &'static str,
    pub task: &'static str,
    pub weak_topics_empty: &'static str,
    pub next_exercise: &'static str,
    pub finish: &'static str,
    pub start_session: &'static str,
    pub review: &'static str,
    pub docs: &'static str,
    pub curriculum: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
    pub pairs: &'static str,
    pub loading: &'static str,
    pub analyzing: &'static str,
    pub error: &'static str,
    pub retry: &'static str,
    pub no_weak_topics: &'static str,
    pub course_progress: &'static str,
    pub difficulty_progress: &'static str,
    pub session_trend: &'static str,
    pub activity: &'static str,
    pub profile: &'static str,
    pub progress: &'static str,
    pub no_topics: &'static str,
    pub new_label: &'static str,
    pub new_topic_label: &'static str,
    pub in_progress_label: &'static str,
    pub completed_label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewLabels {
    pub select_topic: &'static str,
    pub sort_by_score: &'static str,
    pub sort_by_last_practiced: &'static str,
    pub start_review: &'static str,
    pub no_weak_topics: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocsLabels {
    pub select_topic: &'static str,
    pub theory: &'static str,
    pub practice: &'static str,
    pub regenerate: &'static str,
    pub loading: &'static str,
    pub no_review: &'static str,
}

const EN_REPORT: ReportLabels = ReportLabels {
    translate: "Translate",
    loading_exercises: "Generating exercises...",
    loading_analysis: "Analyzing answers...",
    loading_curriculum: "Generating curriculum...",
    no_exercise: "No exercise available",
    select_topic: "Select Topic",
    choose_topic: "Choose a topic to practice",
    your_answer: "Your answer",
    submit: "submit",
    back: "back",
    cancel: "cancel",
    navigate: "navigate",
    per_exercise_results: "Per-exercise results",
    topic_scores: "Topic scores",
    no_errors: "No errors",
    session_report: "Session Report",
    your_translation: "Your translation",
    correct_answer: "Correct answer",
    feedback: "Feedback",
    score: "Score",
    weak_topics: "Weak topics",
    task: "Task",
    weak_topics_empty: "Start a session to see weak topics.",
    next_exercise: "Next exercise",
    finish: "Finish",
    start_session: "Start session",
    review: "Review",
    docs: "Docs",
    curriculum: "Curriculum",
    settings: "Settings",
    quit: "Quit",
    pairs: "Pairs",
    loading: "Loading...",
    analyzing: "Analyzing...",
    error: "Error",
    retry: "Retry",
    no_weak_topics: "No weak topics yet. Great job!",
    course_progress: "Course Progress",
    difficulty_progress: "Difficulty Progress",
    session_trend: "Session Trend",
    activity: "Activity",
    profile: "Profile",
    progress: "Progress",
    no_topics: "No topics yet. Generate a curriculum from the dashboard.",
    new_label: "new",
    new_topic_label: "added",
    in_progress_label: "in progress",
    completed_label: "completed",
};

const RU_REPORT: ReportLabels = ReportLabels {
    translate: "Перевод",
    loading_exercises: "Генерация упражнений...",
    loading_analysis: "Анализ ответов...",
    loading_curriculum: "Генерация программы...",
    no_exercise: "Нет доступных упражнений",
    select_topic: "Выбор темы",
    choose_topic: "Выберите тему для практики",
    your_answer: "Ваш ответ",
    submit: "ответить",
    back: "назад",
    cancel: "отмена",
    navigate: "навигация",
    per_exercise_results: "Результаты по упражнениям",
    topic_scores: "Оценки тем",
    no_errors: "Без ошибок",
    session_report: "Отчёт о сессии",
    your_translation: "Ваш перевод",
    correct_answer: "Правильный ответ",
    feedback: "Обратная связь",
    score: "Результат",
    weak_topics: "Слабые темы",
    task: "Задание",
    weak_topics_empty: "Начните сессию, чтобы увидеть слабые темы.",
    next_exercise: "Следующее упражнение",
    finish: "Завершить",
    start_session: "Начать сессию",
    review: "Повторение",
    docs: "Документация",
    curriculum: "Программа",
    settings: "Настройки",
    quit: "Выйти",
    pairs: "Пары",
    loading: "Загрузка...",
    analyzing: "Анализ...",
    error: "Ошибка",
    retry: "Повторить",
    no_weak_topics: "Пока нет слабых тем. Отличная работа!",
    course_progress: "Прогресс курса",
    difficulty_progress: "Прогресс по уровням",
    session_trend: "Динамика сессий",
    activity: "Активность",
    profile: "Профиль",
    progress: "Прогресс",
    no_topics: "Тем пока нет. Сначала сгенерируйте программу в меню.",
    new_label: "новых",
    new_topic_label: "добавлена",
    in_progress_label: "в процессе",
    completed_label: "завершено",
};

const EN_REVIEW: ReviewLabels = ReviewLabels {
    select_topic: "Select a topic to review",
    sort_by_score: "Sort by score",
    sort_by_last_practiced: "Sort by last practiced",
    start_review: "Start review",
    no_weak_topics: "No weak topics yet.",
};

const RU_REVIEW: ReviewLabels = ReviewLabels {
    select_topic: "Выберите тему для повторения",
    sort_by_score: "Сортировать по результату",
    sort_by_last_practiced: "Сортировать по последней практике",
    start_review: "Начать повторение",
    no_weak_topics: "Пока нет слабых тем.",
};

const EN_DOCS: DocsLabels = DocsLabels {
    select_topic: "Select a topic",
    theory: "Theory",
    practice: "Practice",
    regenerate: "Regenerate",
    loading: "Loading...",
    no_review: "No review available.",
};

const RU_DOCS: DocsLabels = DocsLabels {
    select_topic: "Выберите тему",
    theory: "Теория",
    practice: "Практика",
    regenerate: "Сгенерировать заново",
    loading: "Загрузка...",
    no_review: "Повторение недоступно.",
};

const SUPPORTED_REPORT: [(&str, ReportLabels); 17] = [
    ("en", EN_REPORT),
    ("ru", RU_REPORT),
    ("es", EN_REPORT),
    ("fr", EN_REPORT),
    ("de", EN_REPORT),
    ("it", EN_REPORT),
    ("pt", EN_REPORT),
    ("zh", EN_REPORT),
    ("ja", EN_REPORT),
    ("ko", EN_REPORT),
    ("ar", EN_REPORT),
    ("hi", EN_REPORT),
    ("tr", EN_REPORT),
    ("pl", EN_REPORT),
    ("nl", EN_REPORT),
    ("sv", EN_REPORT),
    ("uk", EN_REPORT),
];

const SUPPORTED_REVIEW: [(&str, ReviewLabels); 17] = [
    ("en", EN_REVIEW),
    ("ru", RU_REVIEW),
    ("es", EN_REVIEW),
    ("fr", EN_REVIEW),
    ("de", EN_REVIEW),
    ("it", EN_REVIEW),
    ("pt", EN_REVIEW),
    ("zh", EN_REVIEW),
    ("ja", EN_REVIEW),
    ("ko", EN_REVIEW),
    ("ar", EN_REVIEW),
    ("hi", EN_REVIEW),
    ("tr", EN_REVIEW),
    ("pl", EN_REVIEW),
    ("nl", EN_REVIEW),
    ("sv", EN_REVIEW),
    ("uk", EN_REVIEW),
];

const SUPPORTED_DOCS: [(&str, DocsLabels); 17] = [
    ("en", EN_DOCS),
    ("ru", RU_DOCS),
    ("es", EN_DOCS),
    ("fr", EN_DOCS),
    ("de", EN_DOCS),
    ("it", EN_DOCS),
    ("pt", EN_DOCS),
    ("zh", EN_DOCS),
    ("ja", EN_DOCS),
    ("ko", EN_DOCS),
    ("ar", EN_DOCS),
    ("hi", EN_DOCS),
    ("tr", EN_DOCS),
    ("pl", EN_DOCS),
    ("nl", EN_DOCS),
    ("sv", EN_DOCS),
    ("uk", EN_DOCS),
];

fn lookup<T: Copy>(table: &[(&str, T)], lang: &str) -> T {
    table
        .iter()
        .find_map(|(code, labels)| {
            if normalize_language_code(code) == normalize_language_code(lang) {
                Some(*labels)
            } else {
                None
            }
        })
        .unwrap_or(table[0].1)
}

pub fn native_language_code(config: Option<&crate::config::OpenCourseConfig>) -> &str {
    config
        .map(|c| c.active_profile().native_language.as_str())
        .unwrap_or("en")
}

pub fn get_report_labels(lang: &str) -> ReportLabels {
    lookup(&SUPPORTED_REPORT, lang)
}

pub fn get_review_labels(lang: &str) -> ReviewLabels {
    lookup(&SUPPORTED_REVIEW, lang)
}

pub fn get_docs_labels(lang: &str) -> DocsLabels {
    lookup(&SUPPORTED_DOCS, lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_english_by_default() {
        let labels = get_report_labels("unknown");
        assert_eq!(labels.session_report, EN_REPORT.session_report);
    }

    #[test]
    fn returns_russian_for_ru() {
        let labels = get_report_labels("ru");
        assert_eq!(labels.session_report, "Отчёт о сессии");
    }

    #[test]
    fn normalizes_language_code() {
        let labels = get_report_labels("RU");
        assert_eq!(labels.session_report, "Отчёт о сессии");
    }

    #[test]
    fn all_supported_languages_have_report_labels() {
        for (code, _) in SUPPORTED_REPORT {
            assert!(!get_report_labels(code).session_report.is_empty());
        }
    }
}
