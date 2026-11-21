//! pl-classifier – Rust replacement of upstream's scikit-learn document classifier
//! (src/documents/classifier.py@v3.4.2): automatic assignment (MATCH_AUTO) of tags,
//! correspondent, document type and storage path. Pure library: no DB, no routes.
//! See docs/ARCHITECTURE.md and docs/contract/pl-classifier.md.
//!
//! - [`text`] – preprocessing, exactly like upstream (`preprocess_content`)
//! - [`vectorizer`] – CountVectorizer(ngram_range=(1, 2), min_df=0.01), exactly like sklearn
//! - [`mlp`] / [`nb`] – estimators (MLP like sklearn's MLPClassifier, Naive Bayes alternative)
//! - [`model`] – `DocumentClassifier` (train with retrain check, predict, signed model file)
//! - [`metrics`] – quality measure used for the comparison with upstream

pub mod b64;
pub mod metrics;
pub mod mlp;
pub mod model;
pub mod nb;
pub mod rng;
#[rustfmt::skip]
#[allow(dead_code)] // HUNGARIAN kept so the list file stays identical to tantivy's
mod stopwords;
pub mod text;
pub mod vectorizer;

pub use model::{
    DEFAULT_MATCH_THRESHOLD, DocumentClassifier, FORMAT_VERSION, ModelKind, Prediction, Predictor, TrainOptions, TrainingDoc,
    load_classifier, training_hash,
};
pub use text::{Language, preprocess_content, suggestion_content};

#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    /// upstream: ValueError("No training data available.")
    #[error("No training data available.")]
    NoTrainingData,
    /// sklearn: ValueError("empty vocabulary; perhaps the documents only contain stop words")
    #[error("empty vocabulary; perhaps the documents only contain stop words")]
    EmptyVocabulary,
    /// sklearn: ValueError("After pruning, no terms remain. Try a lower min_df or a higher max_df.")
    #[error("After pruning, no terms remain. Try a lower min_df or a higher max_df.")]
    NoTermsRemain,
    /// upstream: OverflowError from int.to_bytes(4, signed=True) in the training hash
    #[error("label id {0} does not fit into 4 bytes")]
    LabelOutOfRange(i64),
    #[error("content provider returned a wrong number of documents")]
    ContentMismatch,
    /// upstream: IncompatibleClassifierVersionError
    #[error("Cannot load classifier, incompatible versions (file format {0}).")]
    IncompatibleVersion(u32),
    /// upstream: ClassifierModelCorruptError
    #[error("classifier model file is corrupt: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(std::io::Error),
}
