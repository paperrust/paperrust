//! Life areas: types, input validation, rule matching (any-of) and suggestions.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Result, invalid};

/// At most this many areas per scope.
pub const MAX_AREAS: usize = 50;
/// At most this many UUIDs per rule list.
pub const MAX_RULE_REFS: usize = 200;

/// Colour palette (accent rust first, docs/UI-ZWEITES-DESIGN.md).
pub const PALETTE: [&str; 10] = [
    "#B7410E", "#2F6B4F", "#3A5BA0", "#8E5A9B", "#C28A00", "#1F7A8C", "#A23B5B", "#5B6B2F",
    "#6B4F3A", "#4F5D75",
];

/// Known icon keys (the UI maps them to icons; others are allowed).
pub const ICONS: [&str; 16] = [
    "home",
    "car",
    "health",
    "money",
    "work",
    "insurance",
    "tax",
    "family",
    "education",
    "travel",
    "pet",
    "phone",
    "shopping",
    "legal",
    "tag",
    "other",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Instance,
}

impl Scope {
    pub fn parse(s: Option<&str>) -> Result<Scope> {
        match s.map(str::trim) {
            None | Some("") | Some("user") => Ok(Scope::User),
            Some("instance") => Ok(Scope::Instance),
            Some(_) => Err(invalid("scope", "Expected user or instance.")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Instance => "instance",
        }
    }
}

/// Rules (object UUIDs, any-of).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    #[serde(default)]
    pub tags: Vec<Uuid>,
    #[serde(default)]
    pub document_types: Vec<Uuid>,
    #[serde(default)]
    pub storage_paths: Vec<Uuid>,
    #[serde(default)]
    pub correspondents: Vec<Uuid>,
}

impl Rules {
    /// (name of the list, sync kind code, list).
    pub fn lists(&self) -> [(&'static str, i16, &Vec<Uuid>); 4] {
        [
            ("tags", 2, &self.tags),
            ("correspondents", 3, &self.correspondents),
            ("document_types", 4, &self.document_types),
            ("storage_paths", 5, &self.storage_paths),
        ]
    }

    fn normalize(&mut self) -> Result<()> {
        for (name, l) in [
            ("tags", &mut self.tags),
            ("document_types", &mut self.document_types),
            ("storage_paths", &mut self.storage_paths),
            ("correspondents", &mut self.correspondents),
        ] {
            let mut seen = HashSet::new();
            l.retain(|u| seen.insert(*u));
            if l.len() > MAX_RULE_REFS {
                return Err(invalid(
                    &format!("rules.{name}"),
                    format!("At most {MAX_RULE_REFS} entries."),
                ));
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.document_types.is_empty()
            && self.storage_paths.is_empty()
            && self.correspondents.is_empty()
    }
}

/// Input (POST/PUT/PATCH).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AreaWrite {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Rules>,
}

/// Stored area.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Area {
    pub uuid: Uuid,
    pub scope: Scope,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub order: i64,
    pub rules: Rules,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    #[serde(skip)]
    pub id: i64,
    #[serde(skip)]
    pub user_id: Option<i64>,
}

/// Checked values of an area.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaValues {
    pub name: String,
    pub color: String,
    pub icon: String,
    pub order: i64,
    pub rules: Rules,
}

fn check_name(n: &str) -> Result<String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 64 {
        return Err(invalid("name", "Name must be 1–64 characters."));
    }
    Ok(n.to_string())
}

fn check_color(c: &str) -> Result<String> {
    let c = c.trim();
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
    if !ok {
        return Err(invalid("color", "Expected #RRGGBB."));
    }
    Ok(c.to_ascii_uppercase())
}

fn check_icon(i: &str) -> Result<String> {
    let i = i.trim();
    let ok = !i.is_empty()
        && i.len() <= 32
        && i.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(invalid("icon", "Expected 1–32 characters a-z, 0-9 or '-'."));
    }
    Ok(i.to_string())
}

impl AreaWrite {
    /// New area at position `pos` (default values for colour/icon/order).
    pub fn values(&self, pos: usize) -> Result<AreaValues> {
        let name = check_name(
            self.name
                .as_deref()
                .ok_or_else(|| invalid("name", "This field is required."))?,
        )?;
        let mut rules = self.rules.clone().unwrap_or_default();
        rules.normalize()?;
        Ok(AreaValues {
            name,
            color: match &self.color {
                Some(c) => check_color(c)?,
                None => PALETTE[pos % PALETTE.len()].to_string(),
            },
            icon: match &self.icon {
                Some(i) => check_icon(i)?,
                None => "tag".into(),
            },
            order: self.order.unwrap_or(pos as i64),
            rules,
        })
    }

    /// Partial change of an existing area.
    pub fn patch(&self, cur: &Area) -> Result<AreaValues> {
        let mut rules = self.rules.clone().unwrap_or_else(|| cur.rules.clone());
        rules.normalize()?;
        Ok(AreaValues {
            name: match &self.name {
                Some(n) => check_name(n)?,
                None => cur.name.clone(),
            },
            color: match &self.color {
                Some(c) => check_color(c)?,
                None => cur.color.clone(),
            },
            icon: match &self.icon {
                Some(i) => check_icon(i)?,
                None => cur.icon.clone(),
            },
            order: self.order.unwrap_or(cur.order),
            rules,
        })
    }
}

/// Rules as legacy ids (resolved via `paperrust_sync_object`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedRules {
    pub tags: HashSet<i64>,
    pub document_types: HashSet<i64>,
    pub storage_paths: HashSet<i64>,
    pub correspondents: HashSet<i64>,
}

/// Assignment attributes of a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocKeys {
    pub id: i64,
    pub tags: Vec<i64>,
    pub document_type: Option<i64>,
    pub storage_path: Option<i64>,
    pub correspondent: Option<i64>,
}

impl ResolvedRules {
    /// any-of: a tag OR type OR storage path OR correspondent.
    pub fn matches(&self, d: &DocKeys) -> bool {
        d.tags.iter().any(|t| self.tags.contains(t))
            || d.document_type
                .is_some_and(|x| self.document_types.contains(&x))
            || d.storage_path
                .is_some_and(|x| self.storage_paths.contains(&x))
            || d.correspondent
                .is_some_and(|x| self.correspondents.contains(&x))
    }
}

// ------------------------------------------------------------------------------ Suggestions

/// A visible object as a candidate for suggestions.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// `tags` | `document_types` | `storage_paths`.
    pub list: &'static str,
    pub id: i64,
    pub uuid: Uuid,
    pub name: String,
    pub is_inbox: bool,
}

/// A suggestion (never stored).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    #[serde(flatten)]
    pub area: AreaWrite,
    pub reason: String,
    pub document_count: usize,
}

struct Category {
    de: &'static str,
    en: &'static str,
    icon: &'static str,
    keywords: &'static [&'static str],
}

const CATEGORIES: &[Category] = &[
    Category {
        de: "Wohnen",
        en: "Home",
        icon: "home",
        keywords: &[
            "wohn",
            "miet",
            "vermiet",
            "haus",
            "nebenkosten",
            "strom",
            "heiz",
            "hausrat",
            "immobil",
            "rent",
            "home",
            "house",
            "utilit",
            "apartment",
        ],
    },
    Category {
        de: "Finanzen",
        en: "Finances",
        icon: "money",
        keywords: &[
            "bank", "konto", "kredit", "finanz", "depot", "invest", "spar", "hypothek", "account",
            "loan", "credit", "finance", "mortgage",
        ],
    },
    Category {
        de: "Versicherungen",
        en: "Insurance",
        icon: "insurance",
        keywords: &["versicher", "police", "insurance", "policy"],
    },
    Category {
        de: "Steuern",
        en: "Taxes",
        icon: "tax",
        keywords: &["steuer", "tax", "finanzamt", "mwst", "vat"],
    },
    Category {
        de: "Gesundheit",
        en: "Health",
        icon: "health",
        keywords: &[
            "gesund", "arzt", "aerzt", "kranken", "spital", "klinik", "apotheke", "zahn",
            "medizin", "health", "doctor", "medical", "hospital", "pharma", "dental",
        ],
    },
    Category {
        de: "Fahrzeug",
        en: "Vehicle",
        icon: "car",
        keywords: &[
            "auto", "fahrzeug", "kfz", "garage", "leasing", "car", "vehicle", "motor",
        ],
    },
    Category {
        de: "Arbeit",
        en: "Work",
        icon: "work",
        keywords: &[
            "arbeit", "lohn", "gehalt", "salaer", "job", "work", "salary", "payroll", "employ",
        ],
    },
    Category {
        de: "Verträge & Abos",
        en: "Contracts & subscriptions",
        icon: "phone",
        keywords: &[
            "vertrag",
            "vertraeg",
            "abo",
            "telefon",
            "handy",
            "mobil",
            "internet",
            "contract",
            "subscription",
            "phone",
            "mobile",
        ],
    },
    Category {
        de: "Familie",
        en: "Family",
        icon: "family",
        keywords: &["famil", "kind", "kinder", "family", "child", "kids"],
    },
    Category {
        de: "Bildung",
        en: "Education",
        icon: "education",
        keywords: &[
            "schul",
            "studi",
            "ausbildung",
            "kurs",
            "zeugnis",
            "school",
            "universit",
            "course",
            "education",
        ],
    },
];

fn fold(s: &str) -> String {
    s.to_lowercase()
        .replace('ä', "ae")
        .replace('ö', "oe")
        .replace('ü', "ue")
        .replace('ß', "ss")
}

fn hits(name: &str, kw: &str) -> bool {
    let n = fold(name);
    if kw.chars().count() < 5 {
        n.split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(kw))
    } else {
        n.contains(kw)
    }
}

fn count(rules: &ResolvedRules, docs: &[DocKeys]) -> usize {
    docs.iter().filter(|d| rules.matches(d)).count()
}

fn add(r: &mut ResolvedRules, w: &mut Rules, c: &Candidate) {
    match c.list {
        "tags" => {
            r.tags.insert(c.id);
            w.tags.push(c.uuid);
        }
        "document_types" => {
            r.document_types.insert(c.id);
            w.document_types.push(c.uuid);
        }
        _ => {
            r.storage_paths.insert(c.id);
            w.storage_paths.push(c.uuid);
        }
    }
}

/// Suggestions from visible objects and documents (at most 8, by document count).
pub fn suggest(cands: &[Candidate], docs: &[DocKeys], english: bool) -> Vec<Suggestion> {
    // Document count per object
    let mut per: BTreeMap<(&str, i64), usize> = BTreeMap::new();
    for d in docs {
        for t in &d.tags {
            *per.entry(("tags", *t)).or_default() += 1;
        }
        if let Some(x) = d.document_type {
            *per.entry(("document_types", x)).or_default() += 1;
        }
        if let Some(x) = d.storage_path {
            *per.entry(("storage_paths", x)).or_default() += 1;
        }
    }
    let n_of = |c: &Candidate| per.get(&(c.list, c.id)).copied().unwrap_or(0);
    let usable: Vec<&Candidate> = cands
        .iter()
        .filter(|c| !c.is_inbox && n_of(c) > 0)
        .collect();
    let mut used: HashSet<(&str, i64)> = HashSet::new();
    let mut out: Vec<Suggestion> = Vec::new();
    for cat in CATEGORIES {
        let mut r = ResolvedRules::default();
        let mut w = Rules::default();
        let mut names = Vec::new();
        for c in &usable {
            if !used.contains(&(c.list, c.id)) && cat.keywords.iter().any(|k| hits(&c.name, k)) {
                add(&mut r, &mut w, c);
                names.push(c.name.clone());
                used.insert((c.list, c.id));
            }
        }
        if names.is_empty() {
            continue;
        }
        let n = count(&r, docs);
        out.push(Suggestion {
            area: AreaWrite {
                name: Some(if english { cat.en } else { cat.de }.to_string()),
                icon: Some(cat.icon.to_string()),
                rules: Some(w),
                ..Default::default()
            },
            reason: format!(
                "{}: {}",
                if english { "matches" } else { "passt zu" },
                names.join(", ")
            ),
            document_count: n,
        });
    }
    if out.len() < 4 {
        let mut tags: Vec<&&Candidate> = usable
            .iter()
            .filter(|c| c.list == "tags" && !used.contains(&(c.list, c.id)) && n_of(c) >= 3)
            .collect();
        tags.sort_by(|a, b| n_of(b).cmp(&n_of(a)).then_with(|| a.name.cmp(&b.name)));
        for c in tags {
            if out.len() >= 8 {
                break;
            }
            out.push(Suggestion {
                area: AreaWrite {
                    name: Some(c.name.chars().take(64).collect()),
                    icon: Some("tag".into()),
                    rules: Some(Rules {
                        tags: vec![c.uuid],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                reason: format!(
                    "{} ({} {})",
                    if english {
                        "frequent tag"
                    } else {
                        "häufiger Tag"
                    },
                    n_of(c),
                    if english { "documents" } else { "Dokumente" }
                ),
                document_count: n_of(c),
            });
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.document_count));
    out.truncate(8);
    for (i, s) in out.iter_mut().enumerate() {
        s.area.order = Some(i as i64);
        s.area.color = Some(PALETTE[i % PALETTE.len()].to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn validation() {
        let w: AreaWrite = serde_json::from_value(
            serde_json::json!({"name":"  Wohnen ","rules":{"tags":[u(1),u(1),u(2)]}}),
        )
        .unwrap();
        let v = w.values(1).unwrap();
        assert_eq!(v.name, "Wohnen");
        assert_eq!(v.color, PALETTE[1]);
        assert_eq!(v.icon, "tag");
        assert_eq!(v.order, 1);
        assert_eq!(v.rules.tags, vec![u(1), u(2)], "Doppelte entfernt");
        for bad in [
            serde_json::json!({"name":""}),
            serde_json::json!({"name":"x","color":"red"}),
            serde_json::json!({"name":"x","icon":"Haus!"}),
            serde_json::json!({}),
        ] {
            let w: AreaWrite = serde_json::from_value(bad.clone()).unwrap();
            assert!(w.values(0).is_err(), "{bad}");
        }
        assert!(
            serde_json::from_value::<AreaWrite>(serde_json::json!({"name":"x","bogus":1})).is_err()
        );
        assert!(
            serde_json::from_value::<AreaWrite>(
                serde_json::json!({"name":"x","rules":{"labels":[]}})
            )
            .is_err()
        );
        let many: Vec<Uuid> = (0..201).map(u).collect();
        let w = AreaWrite {
            name: Some("x".into()),
            rules: Some(Rules {
                tags: many,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(w.values(0).is_err());
        assert_eq!(Scope::parse(None).unwrap(), Scope::User);
        assert!(Scope::parse(Some("all")).is_err());
    }

    #[test]
    fn patch_keeps_values() {
        let cur = Area {
            uuid: u(9),
            scope: Scope::User,
            name: "A".into(),
            color: "#000000".into(),
            icon: "home".into(),
            order: 3,
            rules: Rules {
                tags: vec![u(1)],
                ..Default::default()
            },
            created: Utc::now(),
            modified: Utc::now(),
            id: 1,
            user_id: Some(2),
        };
        let v = AreaWrite {
            color: Some("#aabbcc".into()),
            ..Default::default()
        }
        .patch(&cur)
        .unwrap();
        assert_eq!(
            (v.name.as_str(), v.color.as_str(), v.icon.as_str(), v.order),
            ("A", "#AABBCC", "home", 3)
        );
        assert_eq!(v.rules.tags, vec![u(1)]);
    }

    #[test]
    fn any_of_matching() {
        let r = ResolvedRules {
            tags: [1].into(),
            document_types: [5].into(),
            correspondents: [9].into(),
            ..Default::default()
        };
        assert!(r.matches(&DocKeys {
            id: 1,
            tags: vec![3, 1],
            ..Default::default()
        }));
        assert!(r.matches(&DocKeys {
            id: 2,
            document_type: Some(5),
            ..Default::default()
        }));
        assert!(r.matches(&DocKeys {
            id: 3,
            correspondent: Some(9),
            ..Default::default()
        }));
        assert!(!r.matches(&DocKeys {
            id: 4,
            tags: vec![2],
            storage_path: Some(1),
            correspondent: Some(8),
            ..Default::default()
        }));
        assert!(
            !ResolvedRules::default().matches(&DocKeys {
                id: 5,
                tags: vec![1],
                ..Default::default()
            }),
            "leere Regeln treffen nichts"
        );
    }

    #[test]
    fn suggestions() {
        let c = |list, id, name: &str, inbox| Candidate {
            list,
            id,
            uuid: u(id as u128),
            name: name.into(),
            is_inbox: inbox,
        };
        let cands = vec![
            c("tags", 1, "Versicherung", false),
            c("tags", 2, "Krankenkasse", false),
            c("tags", 3, "Posteingang", true),
            c("tags", 4, "Urlaub", false),
            c("document_types", 10, "Mietvertrag", false),
            c("tags", 5, "Nie benutzt Steuer", false),
            c("tags", 6, "Projekt X", false),
        ];
        let mut docs = Vec::new();
        for i in 0..5 {
            docs.push(DocKeys {
                id: i,
                tags: vec![1, 3],
                ..Default::default()
            });
        }
        docs.push(DocKeys {
            id: 10,
            tags: vec![2],
            ..Default::default()
        });
        docs.push(DocKeys {
            id: 11,
            document_type: Some(10),
            tags: vec![4],
            ..Default::default()
        });
        for i in 20..24 {
            docs.push(DocKeys {
                id: i,
                tags: vec![6],
                ..Default::default()
            });
        }
        let s = suggest(&cands, &docs, false);
        let names: Vec<&str> = s.iter().map(|x| x.area.name.as_deref().unwrap()).collect();
        assert!(names.contains(&"Versicherungen"), "{names:?}");
        assert!(names.contains(&"Gesundheit"), "{names:?}");
        assert!(
            names.contains(&"Wohnen"),
            "Mietvertrag → Wohnen (miete): {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("Steuer")),
            "ohne Dokumente kein Vorschlag"
        );
        assert!(!names.contains(&"Posteingang"));
        assert!(
            names.contains(&"Projekt X"),
            "häufiger Tag aufgefüllt: {names:?}"
        );
        assert!(!names.contains(&"Urlaub"), "weniger als 3 Dokumente");
        assert_eq!(s[0].document_count, 5);
        assert_eq!(s[0].area.order, Some(0));
        assert_eq!(s[0].area.color.as_deref(), Some(PALETTE[0]));
        let en = suggest(&cands, &docs, true);
        assert!(
            en.iter()
                .any(|x| x.area.name.as_deref() == Some("Insurance"))
        );
    }
}
