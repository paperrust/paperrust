// upstream: src/documents/classifier.py@v3.4.2 (DocumentClassifier.train: CountVectorizer(analyzer="word",
//           ngram_range=(1, 2), min_df=0.01))
// sklearn 1.9.1 sklearn/feature_extraction/text.py (CountVectorizer defaults: lowercase=True,
//           token_pattern=r"(?u)\b\w\w+\b", max_df=1.0, max_features=None, binary=False;
//           _word_ngrams, _count_vocab, _sort_features, _limit_features)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ClassifierError;
use crate::text::py_word_runs;

/// Sparse row-major matrix (counts as f32), like scipy CSR.
#[derive(Debug, Clone, Default)]
pub struct Csr {
    pub n_cols: usize,
    pub indptr: Vec<usize>,
    pub indices: Vec<u32>,
    pub data: Vec<f32>,
}

impl Csr {
    pub fn n_rows(&self) -> usize {
        self.indptr.len().saturating_sub(1)
    }

    pub fn row(&self, i: usize) -> (&[u32], &[f32]) {
        let (a, b) = (self.indptr[i], self.indptr[i + 1]);
        (&self.indices[a..b], &self.data[a..b])
    }

    pub fn from_rows(n_cols: usize, rows: &[Vec<(u32, f32)>]) -> Csr {
        let mut m = Csr {
            n_cols,
            indptr: vec![0],
            ..Default::default()
        };
        for r in rows {
            for &(j, v) in r {
                m.indices.push(j);
                m.data.push(v);
            }
            m.indptr.push(m.indices.len());
        }
        m
    }
}

/// CountVectorizer with the parameters upstream uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountVectorizer {
    pub ngram_max: usize,
    pub min_df: f64,
    /// Feature names in column order (sorted by code point like Python `sorted()`).
    pub vocabulary: Vec<String>,
    #[serde(skip)]
    index: HashMap<String, u32>,
}

impl Default for CountVectorizer {
    fn default() -> Self {
        CountVectorizer {
            ngram_max: 2,
            min_df: 0.01,
            vocabulary: Vec::new(),
            index: HashMap::new(),
        }
    }
}

/// sklearn `build_analyzer()` for analyzer="word": `doc.lower()`, token pattern, then
/// unigrams followed by n-grams joined by a space.
fn analyze_doc(doc: &str, ngram_max: usize, mut f: impl FnMut(&str)) {
    let lower = doc.to_lowercase();
    let mut tokens: Vec<&str> = Vec::new();
    py_word_runs(&lower, 2, |t| tokens.push(t));
    for t in &tokens {
        f(t);
    }
    let mut buf = String::new();
    for n in 2..=ngram_max {
        if tokens.len() < n {
            break;
        }
        for w in tokens.windows(n) {
            buf.clear();
            for (k, t) in w.iter().enumerate() {
                if k > 0 {
                    buf.push(' ');
                }
                buf.push_str(t);
            }
            f(&buf);
        }
    }
}

impl CountVectorizer {
    pub fn rebuild_index(&mut self) {
        self.index = self
            .vocabulary
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
    }

    pub fn n_features(&self) -> usize {
        self.vocabulary.len()
    }

    /// `fit_transform`: learns the vocabulary from already preprocessed documents.
    pub fn fit_transform<I: IntoIterator<Item = String>>(
        &mut self,
        docs: I,
    ) -> Result<Csr, ClassifierError> {
        let mut vocab: HashMap<String, u32> = HashMap::new();
        let mut df: Vec<u32> = Vec::new();
        let mut rows: Vec<Vec<(u32, u32)>> = Vec::new();
        let mut counts: HashMap<u32, u32> = HashMap::new();
        for doc in docs {
            counts.clear();
            analyze_doc(&doc, self.ngram_max, |feat| {
                let id = match vocab.get(feat) {
                    Some(&id) => id,
                    None => {
                        let id = df.len() as u32;
                        vocab.insert(feat.to_owned(), id);
                        df.push(0);
                        id
                    }
                };
                *counts.entry(id).or_insert(0) += 1;
            });
            let mut row: Vec<(u32, u32)> = counts.iter().map(|(&k, &v)| (k, v)).collect();
            for &(id, _) in &row {
                df[id as usize] += 1;
            }
            row.sort_unstable();
            rows.push(row);
        }
        if vocab.is_empty() {
            return Err(ClassifierError::EmptyVocabulary);
        }
        let n_doc = rows.len() as f64;
        // _limit_features: dfs <= max_df * n_doc (1.0) and dfs >= min_df * n_doc (float compare)
        let low = self.min_df * n_doc;
        let high = n_doc;
        let mut kept: Vec<(String, u32)> = vocab
            .into_iter()
            .filter(|(_, id)| {
                let d = df[*id as usize] as f64;
                d >= low && d <= high
            })
            .collect();
        if kept.is_empty() {
            return Err(ClassifierError::NoTermsRemain);
        }
        kept.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut remap: Vec<u32> = vec![u32::MAX; df.len()];
        for (new, (_, old)) in kept.iter().enumerate() {
            remap[*old as usize] = new as u32;
        }
        self.vocabulary = kept.into_iter().map(|(t, _)| t).collect();
        self.rebuild_index();
        let mut m = Csr {
            n_cols: self.vocabulary.len(),
            indptr: vec![0],
            ..Default::default()
        };
        let mut tmp: Vec<(u32, f32)> = Vec::new();
        for row in rows {
            tmp.clear();
            tmp.extend(
                row.into_iter()
                    .filter(|(id, _)| remap[*id as usize] != u32::MAX)
                    .map(|(id, c)| (remap[id as usize], c as f32)),
            );
            tmp.sort_unstable_by_key(|x| x.0);
            for &(j, v) in &tmp {
                m.indices.push(j);
                m.data.push(v);
            }
            m.indptr.push(m.indices.len());
        }
        Ok(m)
    }

    /// `transform([doc])` for one preprocessed document: (column, count) sorted by column.
    pub fn transform_one(&self, doc: &str) -> Vec<(u32, f32)> {
        let mut counts: HashMap<u32, f32> = HashMap::new();
        analyze_doc(doc, self.ngram_max, |feat| {
            if let Some(&id) = self.index.get(feat) {
                *counts.entry(id).or_insert(0.0) += 1.0;
            }
        });
        let mut row: Vec<(u32, f32)> = counts.into_iter().collect();
        row.sort_unstable_by_key(|x| x.0);
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_corpus_keeps_all_terms_and_sorts() {
        let mut v = CountVectorizer::default();
        let m = v
            .fit_transform(["b a a".to_string(), "Cc b".to_string()])
            .unwrap();
        // single-char tokens are dropped by the default token pattern
        assert_eq!(v.vocabulary, vec!["cc"]);
        assert_eq!(m.n_rows(), 2);
        let mut v = CountVectorizer::default();
        let m = v
            .fit_transform(["zz yy zz".to_string(), "yy xx".to_string()])
            .unwrap();
        assert_eq!(
            v.vocabulary,
            vec!["xx", "yy", "yy xx", "yy zz", "zz", "zz yy"]
        );
        assert_eq!(
            m.row(0),
            (&[1u32, 3, 4, 5][..], &[1.0f32, 1.0, 2.0, 1.0][..])
        );
        assert_eq!(v.transform_one("zz zz qq"), vec![(4, 2.0)]);
    }

    #[test]
    fn empty_vocabulary_errors() {
        let mut v = CountVectorizer::default();
        assert!(matches!(
            v.fit_transform(["a b c".to_string()]),
            Err(ClassifierError::EmptyVocabulary)
        ));
    }

    #[test]
    fn min_df_float_compare() {
        // min_df * n_doc = 7.0 for 700 documents: 7 documents are enough, 6 are not
        let mut docs: Vec<String> = (0..700).map(|i| format!("common w{i:04}")).collect();
        for d in docs.iter_mut().take(6) {
            d.push_str(" six");
        }
        for d in docs.iter_mut().take(7) {
            d.push_str(" seven");
        }
        let mut v = CountVectorizer::default();
        v.fit_transform(docs).unwrap();
        assert!(v.vocabulary.contains(&"seven".to_string()));
        assert!(!v.vocabulary.contains(&"six".to_string()));
    }
}
