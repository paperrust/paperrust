//! AI options that are not yet in the settings registry (pl-settings) – stored in
//! `paperrust_ai_option` (one JSON value per key), edited via `/api/x-ai/options/`
//! (superuser). Missing keys → defaults here. Environment variables of the same name
//! (`PAPERRUST_AI_VISION_*` …) count as the default if the table has nothing.

use serde::{Deserialize, Serialize};

use crate::vision::VisionLimits;

/// What happens to the trigger tag after processing (`PAPERRUST_AI_AUTO_TRIGGER_TAG`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum TagAfter {
    /// Remove the tag (default, previous behavior).
    #[default]
    Remove,
    /// Remove the tag and set tag `tag` (e.g. "AI done"; created if needed).
    Replace { tag: String },
    /// Leave the tag in place (note: the document is then processed again on every run –
    /// only useful together with "only documents without AI suggestion"; the run therefore skips
    /// documents that already have a review-list entry).
    Keep,
}

/// All options (defaults = "off" or cautious).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiOptions {
    /// Fill custom fields via AI.
    pub custom_fields_enabled: bool,
    /// IDs of the custom fields that the AI may fill (can be enabled per field).
    pub custom_fields: Vec<i64>,
    /// AI text recognition available as a selectable OCR option.
    pub vision_enabled: bool,
    /// Image model (empty = text model from `PAPERLESS_AI_LLM_MODEL`).
    pub vision_model: String,
    /// When: `off` (only on request per document), `difficult` (only if the normal text recognition
    /// yields little/no text), `always`.
    pub vision_when: String,
    /// Limits (pages per document, image size, pages per day, tokens per page).
    pub vision_limits: VisionLimits,
    /// Behavior of the trigger tag.
    pub tag_after: TagAfter,
    /// Post-processing: documents per minute (rate limit, 1…120).
    pub backfill_per_minute: u32,
    /// Post-processing: at most this many documents per job.
    pub backfill_max_documents: u32,
}

impl Default for AiOptions {
    fn default() -> Self {
        Self {
            custom_fields_enabled: false,
            custom_fields: Vec::new(),
            vision_enabled: false,
            vision_model: String::new(),
            vision_when: "difficult".into(),
            vision_limits: VisionLimits::default(),
            tag_after: TagAfter::Remove,
            backfill_per_minute: 6,
            backfill_max_documents: 2000,
        }
    }
}

/// Known keys (table `paperrust_ai_option.key` = field name).
pub const KEYS: &[&str] = &[
    "custom_fields_enabled",
    "custom_fields",
    "vision_enabled",
    "vision_model",
    "vision_when",
    "vision_limits",
    "tag_after",
    "backfill_per_minute",
    "backfill_max_documents",
];

impl AiOptions {
    /// From key/value pairs (table) over defaults from the environment. Unreadable values
    /// are ignored (default stays).
    pub fn from_pairs(env: &dyn Fn(&str) -> Option<String>, pairs: &[(String, serde_json::Value)]) -> Self {
        let mut o = Self::default();
        let b = |k: &str| env(k).and_then(|v| crate::config::str_to_bool(&v).ok());
        let n = |k: &str| env(k).and_then(|v| v.trim().parse::<u32>().ok());
        if let Some(v) = b("PAPERRUST_AI_VISION_ENABLED") {
            o.vision_enabled = v;
        }
        if let Some(v) = env("PAPERRUST_AI_VISION_MODEL") {
            o.vision_model = v;
        }
        if let Some(v) = env("PAPERRUST_AI_VISION_WHEN") {
            o.vision_when = v;
        }
        if let Some(v) = n("PAPERRUST_AI_VISION_MAX_PAGES") {
            o.vision_limits.max_pages = v;
        }
        if let Some(v) = n("PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY") {
            o.vision_limits.max_pages_per_day = v;
        }
        if let Some(v) = b("PAPERRUST_AI_CUSTOM_FIELDS_ENABLED") {
            o.custom_fields_enabled = v;
        }
        let mut obj = serde_json::to_value(&o).unwrap_or(serde_json::Value::Null);
        if let Some(m) = obj.as_object_mut() {
            for (k, v) in pairs {
                if KEYS.contains(&k.as_str()) {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        let mut out: Self = serde_json::from_value(obj).unwrap_or(o);
        out.sanitize();
        out
    }

    /// Bring values into allowed ranges.
    pub fn sanitize(&mut self) {
        if !matches!(self.vision_when.as_str(), "off" | "difficult" | "always") {
            self.vision_when = "difficult".into();
        }
        self.vision_model = self.vision_model.trim().chars().take(128).collect();
        self.vision_limits.max_pages = self.vision_limits.max_pages.clamp(1, 200);
        self.vision_limits.max_image_bytes = self.vision_limits.max_image_bytes.clamp(64 * 1024, 32 * 1024 * 1024);
        self.vision_limits.max_tokens_per_page = self.vision_limits.max_tokens_per_page.min(32_768);
        self.backfill_per_minute = self.backfill_per_minute.clamp(1, 120);
        self.backfill_max_documents = self.backfill_max_documents.clamp(1, 100_000);
        self.custom_fields.sort_unstable();
        self.custom_fields.dedup();
        if let TagAfter::Replace { tag } = &mut self.tag_after {
            *tag = tag.trim().chars().take(128).collect();
            if tag.is_empty() {
                self.tag_after = TagAfter::Remove;
            }
        }
    }

    /// Checks a single key/value before saving (error = reason).
    pub fn validate_pair(key: &str, value: &serde_json::Value) -> Result<serde_json::Value, String> {
        if !KEYS.contains(&key) {
            return Err("unknown option".into());
        }
        let mut obj = serde_json::to_value(Self::default()).map_err(|e| e.to_string())?;
        if let Some(m) = obj.as_object_mut() {
            m.insert(key.to_string(), value.clone());
        }
        let mut o: Self = serde_json::from_value(obj).map_err(|_| "invalid value".to_string())?;
        o.sanitize();
        let back = serde_json::to_value(&o).map_err(|e| e.to_string())?;
        back.get(key).cloned().ok_or_else(|| "invalid value".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_overrides() {
        let none = |_: &str| None;
        let o = AiOptions::from_pairs(&none, &[]);
        assert_eq!(o, AiOptions::default());
        assert!(!o.vision_enabled && !o.custom_fields_enabled);
        let env = |k: &str| (k == "PAPERRUST_AI_VISION_ENABLED").then(|| "yes".to_string());
        let o = AiOptions::from_pairs(&env, &[("vision_model".into(), json!(" llava ")), ("unknown".into(), json!(1)), ("backfill_per_minute".into(), json!(1000))]);
        assert!(o.vision_enabled);
        assert_eq!(o.vision_model, "llava");
        assert_eq!(o.backfill_per_minute, 120);
        // unreadable value → default stays
        let o = AiOptions::from_pairs(&none, &[("custom_fields".into(), json!("x"))]);
        assert_eq!(o, AiOptions::default());
    }

    #[test]
    fn validate_pairs() {
        assert_eq!(AiOptions::validate_pair("tag_after", &json!({"action": "replace", "tag": " KI erledigt "})), Ok(json!({"action": "replace", "tag": "KI erledigt"})));
        assert_eq!(AiOptions::validate_pair("tag_after", &json!({"action": "replace", "tag": ""})), Ok(json!({"action": "remove"})));
        assert!(AiOptions::validate_pair("tag_after", &json!({"action": "explode"})).is_err());
        assert!(AiOptions::validate_pair("nope", &json!(1)).is_err());
        assert_eq!(AiOptions::validate_pair("custom_fields", &json!([3, 1, 3])), Ok(json!([1, 3])));
        assert_eq!(AiOptions::validate_pair("vision_when", &json!("sometimes")), Ok(json!("difficult")));
    }
}
