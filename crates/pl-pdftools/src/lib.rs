//! pl-pdftools – PDF tools redact, shrink and repair based on PdfCraft
//! (https://github.com/storytold/pdfcraft, MIT OR Apache-2.0; see NOTICE), isolated in a
//! child process (docs/contract/pl-pdftools.md).
//!
//! * [`job`]: job format (JSON) between the server and `paperrust-pdftool`.
//! * [`runner`]: server side – start the child process, time and memory limit, read the reply.
//! * [`matching`]: pure search logic (term → word rectangles).
//! * `engine` (feature `engine`, child process only): the PdfCraft calls.
//! * [`guard`]: counting allocator for the memory limit of the child process.
//! * `office` (feature `office`, part of `engine`, child process only): office/text → PDF with
//!   WordCraft (https://github.com/storytold/wordcraft, MIT OR Apache-2.0; see NOTICE), also
//!   old Word (.doc) and spreadsheets (.xlsx/.xls/.ods), slides (built by the server as HTML).
//! * `heif` (feature `heif`, part of `engine`, child process only): HEIC → PNG with craft-heif.
//!
//! According to its own README PdfCraft is alpha software, fuzzing still finds crashes and hangs.
//! Therefore it **never** runs in the server process.

pub mod guard;
pub mod job;
pub mod matching;
pub mod runner;

#[cfg(feature = "engine")]
pub mod engine;

#[cfg(feature = "engine")]
pub mod scanimages;

#[cfg(feature = "office")]
pub mod office;

#[cfg(feature = "heif")]
pub mod heif;

pub use job::{Area, ConvertReport, Hit, Op, RedactReport, RepairReport, Reply, ShrinkPreset, ShrinkReport};
pub use runner::{Limits, RunError};
