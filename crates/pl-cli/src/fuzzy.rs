//! `document_fuzzy_match [--ratio R] [--delete] [--yes] [--url URL] [--processes N]` (B2).
//!
//! upstream: src/documents/management/commands/document_fuzzy_match.py@v3.4.2 (`_process_and_match`,
//! `Command.handle`, `_render_results`), rapidfuzz 3.14 (`utils.default_process`, `fuzz.ratio`
//! = normalized Indel similarity × 100 with `score_cutoff`).
//!
//! Flow as upstream: contents of all documents (`Document.objects`, without trash, by id),
//! each pair exactly once (`itertools.combinations`), pairs with empty content (`strip()`) are
//! skipped; hits ≥ threshold in descending order of similarity; per pair the second document
//! (`doc_two_pk`) is marked for deletion; `--delete` asks (except with `--yes`) "[y/N]" and then puts the
//! documents into the trash (`QuerySet.delete()` of the soft-delete manager).
//! Output as text instead of a Rich table (known difference).

use std::io::Write as _;

use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    args::PROCESSES,
    value(&["--ratio"], "ratio"),
    flag(&["--delete"], "delete"),
    flag(&["--yes"], "yes"),
    value(&["--url"], "url"),
];

// ------------------------------------------------------------------------------------------
// rapidfuzz
// ------------------------------------------------------------------------------------------

/// `rapidfuzz.utils.default_process`: every non-alphanumeric character → space,
/// lowercase, whitespace at the ends removed.
pub fn default_process(s: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            // simple mapping (one character) like rapidfuzz's table
            out.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            out.push(' ');
        }
    }
    let start = out.iter().position(|c| !c.is_whitespace()).unwrap_or(out.len());
    let end = out.iter().rposition(|c| !c.is_whitespace()).map_or(start, |e| e + 1);
    out[start..end].to_vec()
}

/// `rapidfuzz.fuzz.ratio(s1, s2, score_cutoff=cutoff)` on already processed strings
/// (shared version in pl-matching).
pub use pl_matching::fuzz::ratio;

// ------------------------------------------------------------------------------------------
// Command
// ------------------------------------------------------------------------------------------

/// `_WorkResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub doc_one_pk: i64,
    pub doc_two_pk: i64,
    pub ratio: f64,
}

/// All pairs (`combinations`) with `ratio ≥ opt_ratio`, sorted in descending order (stable like
/// Python's `sorted(..., reverse=True)` on generation order).
pub async fn find_matches(docs: Vec<(i64, String)>, opt_ratio: f64, processes: usize) -> Vec<Match> {
    let processed: std::sync::Arc<Vec<(i64, bool, Vec<char>)>> =
        std::sync::Arc::new(docs.into_iter().map(|(pk, c)| (pk, !c.trim().is_empty(), default_process(&c))).collect());
    let n = processed.len();
    // split the work into row blocks (first document of the pair); the order is preserved.
    let rows: Vec<usize> = (0..n).collect();
    let mut per_row: Vec<(usize, Vec<Match>)> = Vec::new();
    crate::par::process_parallel(
        rows,
        processes,
        |i: usize| {
            let p = processed.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut out = Vec::new();
                    let (pk_a, ok_a, a) = &p[i];
                    if !*ok_a {
                        return out;
                    }
                    for (pk_b, ok_b, b) in p.iter().skip(i + 1) {
                        if !*ok_b {
                            continue;
                        }
                        let r = ratio(a, b, opt_ratio);
                        if r >= opt_ratio {
                            out.push(Match { doc_one_pk: *pk_a, doc_two_pk: *pk_b, ratio: r });
                        }
                    }
                    out
                })
                .await
                .map_err(|e| e.to_string())
            }
        },
        |i, r| match r {
            Ok(v) => per_row.push((i, v)),
            Err(e) => println!("Failed: {e}"),
        },
    )
    .await;
    per_row.sort_by_key(|(i, _)| *i);
    let mut all: Vec<Match> = per_row.into_iter().flat_map(|(_, v)| v).collect();
    all.sort_by(|x, y| y.ratio.partial_cmp(&x.ratio).unwrap_or(std::cmp::Ordering::Equal));
    all
}

/// `_render_results` as text; returns: marked ids (`doc_two_pk` per pair).
fn render(
    matches: &[Match],
    titles: &std::collections::HashMap<i64, String>,
    opt_ratio: f64,
    do_delete: bool,
    base_url: Option<&str>,
) -> Vec<i64> {
    if matches.is_empty() {
        println!("No duplicate documents found.");
        return Vec::new();
    }
    let cell = |pk: i64| match base_url {
        Some(u) => format!("{}/documents/{pk}/details", u.trim_end_matches('/')),
        None => format!("#{pk} {}", titles.get(&pk).map(String::as_str).unwrap_or("Unknown")),
    };
    println!("Fuzzy Matches (threshold: {opt_ratio:.1}%)");
    let mut ids = Vec::new();
    for (i, m) in matches.iter().enumerate() {
        println!("{:>4}  {}  |  {}  |  {:.1}%", i + 1, cell(m.doc_one_pk), cell(m.doc_two_pk), m.ratio);
        ids.push(m.doc_two_pk);
    }
    let mut summary = format!("Found {} matching pair(s).", matches.len());
    if do_delete {
        summary.push_str(&format!(" {} document(s) will be deleted.", ids.len()));
    }
    println!("{summary}");
    ids
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let processes = crate::par::processes(&a)?;
    let opt_ratio = match a.value("ratio") {
        None => 85.0,
        Some(v) => v.trim().parse::<f64>().map_err(|_| CliError::Usage(format!("argument --ratio: invalid float value: '{v}'")))?,
    };
    if !(0.0..=100.0).contains(&opt_ratio) {
        return Err(CliError::Command("The ratio must be between 0 and 100".into()));
    }
    let do_delete = a.flag("delete");
    if do_delete {
        println!("WARNING: This run is configured to delete documents. One document from each matched pair WILL BE PERMANENTLY DELETED.");
    }
    let c = Ctx::connect().await?;
    let rows: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, content FROM documents_document WHERE deleted_at IS NULL ORDER BY id").fetch_all(&c.pool).await?;
    let docs: Vec<(i64, String)> = rows.into_iter().map(|(i, s)| (i64::from(i), s)).collect();
    let matches = find_matches(docs, opt_ratio, processes).await;
    let base_url = a.value("url").filter(|u| !u.is_empty());
    let mut titles = std::collections::HashMap::new();
    if base_url.is_none() && !matches.is_empty() {
        let pks: Vec<i32> = matches.iter().flat_map(|m| [m.doc_one_pk, m.doc_two_pk]).filter_map(|p| i32::try_from(p).ok()).collect();
        let t: Vec<(i32, String)> =
            sqlx::query_as("SELECT id, title FROM documents_document WHERE id = ANY($1)").bind(&pks).fetch_all(&c.pool).await?;
        titles = t.into_iter().map(|(i, s)| (i64::from(i), s)).collect();
    }
    let ids = render(&matches, &titles, opt_ratio, do_delete, base_url);
    if do_delete && !ids.is_empty() {
        let confirmed = if a.flag("yes") {
            true
        } else {
            print!("\nDelete {} document(s)? [y/N] ", ids.len());
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
        };
        if confirmed {
            println!("Deleting {} document(s)...", ids.len());
            delete_documents(&c, &ids).await?;
            println!("Done.");
        } else {
            println!("Deletion cancelled.");
        }
    }
    Ok(())
}

/// `Document.objects.filter(pk__in=ids).delete()` – soft delete (trash) with cascade and
/// audit log like pl-documents-write (one transaction; duplicate ids only once).
async fn delete_documents(c: &Ctx, ids: &[i64]) -> CliResult {
    let media = pl_documents_write::files::MediaCfg::from_config(&c.cfg);
    let log = pl_documents_write::auditlog::LogCtx { enabled: c.cfg.audit_log_enabled, ..Default::default() };
    let mut seen = std::collections::BTreeSet::new();
    let mut tx = c.pool.begin().await?;
    for id in ids {
        if !seen.insert(*id) {
            continue;
        }
        pl_documents_write::softdelete::soft_delete_document(&mut tx, &log, &media, *id, Some(uuid::Uuid::new_v4()))
            .await
            .map_err(|e| CliError::Command(format!("deleting document {id} failed: {e:?}")))?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: &str, b: &str) -> f64 {
        ratio(&default_process(a), &default_process(b), 0.0)
    }

    #[test]
    fn default_process_like_rapidfuzz() {
        let s: String = default_process("  Hello, World!  ").into_iter().collect();
        assert_eq!(s, "hello  world");
        let s: String = default_process("ÄÖÜ-ß 12").into_iter().collect();
        assert_eq!(s, "äöü ß 12");
        assert!(default_process("!!!").is_empty());
    }

    #[test]
    fn ratio_matches_rapidfuzz_reference_values() {
        // rapidfuzz.fuzz.ratio("this is a test", "this is a test!") == 96.55172413793103
        assert!(
            (ratio(&"this is a test".chars().collect::<Vec<_>>(), &"this is a test!".chars().collect::<Vec<_>>(), 0.0)
                - 96.551_724_137_931_03)
                .abs()
                < 1e-9
        );
        // fuzz.ratio("lewenstein", "levenshtein") == 85.71428571428572
        assert!(
            (ratio(&"lewenstein".chars().collect::<Vec<_>>(), &"levenshtein".chars().collect::<Vec<_>>(), 0.0) - 85.714_285_714_285_72)
                .abs()
                < 1e-9
        );
        assert_eq!(r("abc", "abc"), 100.0);
        assert_eq!(r("", ""), 100.0);
        assert_eq!(r("abc", "xyz"), 0.0);
        // score_cutoff: below it 0
        assert_eq!(ratio(&"lewenstein".chars().collect::<Vec<_>>(), &"levenshtein".chars().collect::<Vec<_>>(), 90.0), 0.0);
        // long strings (> 64 characters, several words in the bit vector)
        let a = "x".repeat(130);
        let b = "xy".repeat(100);
        assert!((r(&a, &b) - 2.0 * 100.0 / 330.0 * 100.0).abs() < 1e-9);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairs_once_skip_empty_sorted_desc() {
        let docs = vec![
            (1, "The quick brown fox jumps over the lazy dog".to_string()),
            (2, "The quick brown fox jumps over the lazy dog!".to_string()),
            (3, "   ".to_string()),
            (4, "the quick brown fox jumped over the lazy dog".to_string()),
            (5, "completely different text about invoices".to_string()),
        ];
        for processes in [1, 3] {
            let m = find_matches(docs.clone(), 85.0, processes).await;
            let pairs: Vec<(i64, i64)> = m.iter().map(|x| (x.doc_one_pk, x.doc_two_pk)).collect();
            assert_eq!(pairs[0], (1, 2), "identical after default_process → 100 first");
            assert_eq!(m[0].ratio, 100.0);
            assert!(pairs.contains(&(1, 4)) && pairs.contains(&(2, 4)));
            assert!(!pairs.iter().any(|(a, b)| *a == 3 || *b == 3 || *a == 5 || *b == 5));
            assert!(m.windows(2).all(|w| w[0].ratio >= w[1].ratio));
        }
    }
}
