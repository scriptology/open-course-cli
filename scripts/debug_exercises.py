#!/usr/bin/env python3
"""Debug script that sends the exercise-generation prompt to the configured LLM.

Reads .open-course-cli/config.json, builds a realistic exercise prompt for the
active language pair, and prints the raw response so we can see why parsing
fails.
"""

import json
import sys
import urllib.request
from pathlib import Path


def load_config(data_dir: Path) -> dict:
    path = data_dir / ".open-course-cli" / "config.json"
    if not path.exists():
        raise FileNotFoundError(f"Config not found: {path}")
    with path.open() as f:
        return json.load(f)


def get_active_provider(config: dict) -> dict:
    provider_id = config["activeProvider"]
    provider = config["providers"][provider_id]
    provider["id"] = provider_id
    return provider


def get_active_profile(config: dict) -> dict:
    active_pair = config["activePair"]
    for pair in config["pairs"]:
        if pair["id"] == active_pair:
            return pair["profile"]
    raise KeyError(f"Active pair {active_pair} not found")


def build_prompt(profile: dict, count: int = 3) -> str:
    native = profile["nativeLanguage"]
    target = profile["targetLanguage"]
    cefr = profile.get("selfAssessedCefr", "A2")
    age = profile.get("age")

    candidate_topics = [
        {"id": "present-simple", "name": "Present Simple"},
        {"id": "articles-basic", "name": "Basic Articles"},
        {"id": "word-order", "name": "Word Order"},
    ]

    topic_list = "\n".join(
        f'- topicId: "{t["id"]}", name: "{t["name"]}"' for t in candidate_topics
    )

    age_hint = ""
    if age:
        age_hint = (
            f"Student age: {age}. Use age-appropriate contexts. Avoid school, "
            "kindergarten, or other child-specific scenarios unless the age makes "
            "them clearly relevant."
        )

    return f"""You are a language tutor. Generate {count} connected translation exercises from {native} to {target}.

Target topics: Present Simple, Basic Articles
Target difficulties: Present Simple (beginner), Basic Articles (beginner)
Side topics: Word Order
Native language: {native}
Proficiency level (self-assessed): {cefr}
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

Output a JSON object with a single key "exercises" containing an array of the exercise objects."""


def send_request(provider: dict, prompt: str) -> dict:
    base_url = provider.get("base_url", "https://api.openai.com/v1")
    if not base_url.endswith("/chat/completions"):
        base_url = base_url.rstrip("/") + "/chat/completions"

    payload = {
        "model": provider["model"],
        "messages": [
            {
                "role": "system",
                "content": (
                    "You are a language tutor. Return ONLY valid JSON matching "
                    "the requested schema. Do not wrap in markdown, do not add "
                    "explanations, do not add commentary."
                ),
            },
            {"role": "user", "content": prompt},
        ],
        "stream": False,
    }

    req = urllib.request.Request(
        base_url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {provider['api_key']}",
            "Content-Type": "application/json",
        },
        method="POST",
    )

    with urllib.request.urlopen(req, timeout=300) as resp:
        return json.loads(resp.read().decode("utf-8"))


def main() -> int:
    data_dir = Path.cwd()
    if len(sys.argv) > 1:
        data_dir = Path(sys.argv[1])

    config = load_config(data_dir)
    provider = get_active_provider(config)
    profile = get_active_profile(config)

    print(f"Provider: {provider['id']}")
    print(f"Model: {provider['model']}")
    print(f"Base URL: {provider.get('base_url', 'default')}")
    print(f"Active pair: {config['activePair']}")
    print(f"Profile: {profile}")
    print()

    prompt = build_prompt(profile)
    print("=" * 60)
    print("PROMPT")
    print("=" * 60)
    print(prompt)
    print()

    response = send_request(provider, prompt)
    raw = response["choices"][0]["message"]["content"]

    print("=" * 60)
    print("RAW RESPONSE")
    print("=" * 60)
    print(raw)
    print()

    print("=" * 60)
    print("JSON PARSE ATTEMPT")
    print("=" * 60)
    try:
        parsed = json.loads(raw)
        print(json.dumps(parsed, ensure_ascii=False, indent=2))
    except json.JSONDecodeError as e:
        print(f"Failed to parse as JSON: {e}")
        # Try stripping markdown fences
        cleaned = raw.strip()
        if cleaned.startswith("```"):
            cleaned = "\n".join(cleaned.split("\n")[1:])
        if cleaned.endswith("```"):
            cleaned = cleaned[:-3].strip()
        try:
            parsed = json.loads(cleaned)
            print("Parsed after stripping fences:")
            print(json.dumps(parsed, ensure_ascii=False, indent=2))
        except json.JSONDecodeError as e2:
            print(f"Still failed after stripping fences: {e2}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
