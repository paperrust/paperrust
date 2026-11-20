//! pl-consumer – control of ingestion (Python-free worker). Contract: docs/contract/pl-consumer.md.
//!
//! upstream: src/documents/consumer.py, src/documents/tasks.py (consume_file),
//! src/documents/data_models.py, src/documents/signals/handlers.py (receivers on add),
//! src/documents/double_sided.py, src/documents/barcodes.py,
//! src/documents/management/commands/document_consumer.py (all @v3.4.2).
//!
//! Entry points: [`Consumer::consume`] / [`Consumer::consume_file`]; task for pl-queue:
//! [`task::handler`]; input folder: [`watcher::FolderWatcher`].

pub mod asn_location;
pub mod audit;
pub mod bulk;
pub mod collate;
pub mod consumer;
pub mod data;
pub mod db;
pub mod docops;
pub mod files;
pub mod lookup;
pub mod manual;
pub mod medialock;
pub mod native;
pub mod office;
pub mod pdftools;
pub mod remote;
pub mod reprocess;
pub mod scheduled;
pub mod settings;
pub mod signals;
pub mod stages;
pub mod task;
pub mod templating;
pub mod watcher;
/// PaperRust: test run of workflows (same core as the real execution).
pub mod workflow_dryrun;
/// PaperRust: history, else branch, AI condition.
pub mod workflow_ext;
pub mod workflows;

pub use consumer::{ConsumeError, Consumer, compute_checksum};
pub use data::{ConsumableDocument, ConsumeOutcome, DocumentMetadataOverrides, DocumentSource};
pub use settings::ConsumerSettings;
