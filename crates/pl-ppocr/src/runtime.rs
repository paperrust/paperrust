//! Inference runtimes behind one small interface, so the pipeline does not depend on a specific
//! ONNX implementation (and the `ort` release candidate stays encapsulated in one module).
//!
//! - [`ort_rt`] (feature `ort`): ONNX Runtime, loaded at run time with `libloading` like PDFium
//!   (no build-time download, no link dependency). Missing library → [`Error::RuntimeMissing`],
//!   callers fall back to Tesseract.
//!
//! The pure-Rust `tract-onnx` was measured in stage 0 (2.3× slower, see
//! `results/ocr-eval/2026-11-25/stage0.md`) and removed.

use std::path::Path;

use crate::{Error, Result};

/// Dense f32 tensor (row-major).
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl Tensor {
    pub fn new(shape: Vec<usize>, data: Vec<f32>) -> Result<Self> {
        if shape.iter().product::<usize>() != data.len() {
            return Err(Error::Model(format!("tensor shape {shape:?} does not match {} values", data.len())));
        }
        Ok(Self { shape, data })
    }
}

/// A loaded model that can be run from several threads.
pub trait Model: Send + Sync {
    /// Runs the model on its single input and returns its first output.
    fn run(&self, input: Tensor) -> Result<Tensor>;
    /// Custom metadata entry of the ONNX file (e.g. `character` of the PP-OCR recognizer).
    fn metadata(&self, key: &str) -> Option<String>;
}

/// Creates [`Model`]s.
pub trait Runtime: Send + Sync {
    fn name(&self) -> String;
    /// Loads a model; `threads` = intra-op threads for one run.
    fn load(&self, path: &Path, threads: usize) -> Result<Box<dyn Model>>;
    /// Loads one model as several sessions with the given intra-op thread counts, so runs can
    /// proceed concurrently (see [`pick_session`]). Default: one session with the first count.
    fn load_pool(&self, path: &Path, threads: &[usize]) -> Result<Box<dyn Model>> {
        self.load(path, threads.first().copied().unwrap_or(1))
    }
}

/// Which free session a new run takes: when nothing else runs, the one with the most threads (a
/// lone page uses all cores); otherwise the free one with the fewest threads (concurrent pages share
/// the cores). `None` = all busy, wait.
pub fn pick_session(threads: &[usize], busy: &[bool]) -> Option<usize> {
    let active = busy.iter().filter(|&&b| b).count();
    let free = (0..threads.len()).filter(|&i| !busy[i]);
    if active == 0 { free.max_by_key(|&i| (threads[i], usize::MAX - i)) } else { free.min_by_key(|&i| (threads[i], i)) }
}

/// Session layout for `cores` and `sessions`: one session with all cores (a lone run) plus
/// `sessions - 1` sessions sharing the cores evenly; `sessions <= 1` = one session with all cores.
pub fn session_threads(cores: usize, sessions: usize) -> Vec<usize> {
    let cores = cores.max(1);
    if sessions <= 1 {
        return vec![cores];
    }
    let small = sessions - 1;
    let mut v = vec![cores];
    v.extend(std::iter::repeat_n((cores / small).max(1), small));
    v
}

#[cfg(feature = "ort")]
pub mod ort_rt {
    //! ONNX Runtime via the `ort` crate with `load-dynamic`.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex, OnceLock};

    use super::{Model, Runtime, Tensor};
    use crate::{Error, Result};

    /// File names tried when no explicit path is configured, in this order.
    pub fn default_library_names() -> &'static [&'static str] {
        if cfg!(windows) {
            &["onnxruntime.dll"]
        } else if cfg!(target_os = "macos") {
            &["libonnxruntime.dylib"]
        } else {
            &["libonnxruntime.so.1", "libonnxruntime.so"]
        }
    }

    static ENV: OnceLock<std::result::Result<(ort::environment::Environment, String), String>> = OnceLock::new();

    /// ONNX Runtime loaded from a shared library. Only the first successfully loaded library is used
    /// per process (ONNX Runtime's API table is global).
    pub struct OrtRuntime {
        library: String,
    }

    impl OrtRuntime {
        /// Loads ONNX Runtime from the first existing candidate (explicit paths first, then the bare
        /// names, which the system loader resolves). Never panics; a missing or too old library is
        /// an error.
        pub fn load(candidates: &[PathBuf]) -> Result<Self> {
            let res = ENV.get_or_init(|| {
                let mut errors = Vec::new();
                let mut list: Vec<PathBuf> = candidates.to_vec();
                list.extend(default_library_names().iter().map(PathBuf::from));
                for c in list {
                    let explicit = c.components().count() > 1;
                    if explicit && !c.is_file() {
                        continue;
                    }
                    match ort::init_from(&c) {
                        Ok(b) => match b.with_name("paperrust").with_telemetry(false).build() {
                            Ok(env) => return Ok((env, c.display().to_string())),
                            Err(e) => errors.push(format!("{}: {e}", c.display())),
                        },
                        Err(e) => errors.push(format!("{}: {e}", c.display())),
                    }
                    // ort keeps the first library it managed to open; do not try further ones.
                    if explicit {
                        break;
                    }
                }
                Err(if errors.is_empty() { "no ONNX Runtime library found".to_string() } else { errors.join("; ") })
            });
            match res {
                Ok((_, lib)) => Ok(Self { library: lib.clone() }),
                Err(e) => Err(Error::RuntimeMissing(e.clone())),
            }
        }

        fn env() -> Result<&'static ort::environment::Environment> {
            match ENV.get() {
                Some(Ok((env, _))) => Ok(env),
                _ => Err(Error::RuntimeMissing("ONNX Runtime not loaded".into())),
            }
        }
    }

    impl Runtime for OrtRuntime {
        fn name(&self) -> String {
            format!("onnxruntime ({})", self.library)
        }

        fn load(&self, path: &Path, threads: usize) -> Result<Box<dyn Model>> {
            self.load_pool(path, &[threads])
        }

        fn load_pool(&self, path: &Path, threads: &[usize]) -> Result<Box<dyn Model>> {
            let threads: Vec<usize> = if threads.is_empty() { vec![1] } else { threads.iter().map(|&t| t.max(1)).collect() };
            let mut sessions = Vec::with_capacity(threads.len());
            for &t in &threads {
                sessions.push(Mutex::new(self.session(path, t)?));
            }
            let mut meta = HashMap::new();
            if let Some(first) = sessions.first() {
                let s = first.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Ok(m) = s.metadata()
                    && let Ok(keys) = m.custom_keys()
                {
                    for k in keys {
                        if let Some(v) = m.custom(&k) {
                            meta.insert(k, v);
                        }
                    }
                }
            }
            let busy = vec![false; threads.len()];
            Ok(Box::new(OrtModel { sessions, threads, busy: Mutex::new(busy), cv: Condvar::new(), meta, path: path.to_path_buf() }))
        }
    }

    impl OrtRuntime {
        fn session(&self, path: &Path, threads: usize) -> Result<ort::session::Session> {
            let env = Self::env()?;
            let err = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", path.display()));
            let session = ort::session::Session::builder(env)
                .map_err(|e| err(&e))?
                .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
                .map_err(|e| err(&e))?
                .with_intra_threads(threads.max(1))
                .map_err(|e| err(&e))?
                .with_inter_threads(1)
                .map_err(|e| err(&e))?
                .with_memory_pattern(true)
                .map_err(|e| err(&e))?
                .with_intra_op_spinning(false)
                .map_err(|e| err(&e))?
                .commit_from_file(path)
                .map_err(|e| err(&e))?;
            Ok(session)
        }
    }

    /// `ort` 2.0-rc needs `&mut Session` per run, so one session runs one page at a time. Several
    /// sessions (`load_pool`) let pages run concurrently; [`super::pick_session`] chooses.
    struct OrtModel {
        sessions: Vec<Mutex<ort::session::Session>>,
        threads: Vec<usize>,
        busy: Mutex<Vec<bool>>,
        cv: Condvar,
        meta: HashMap<String, String>,
        path: PathBuf,
    }

    /// Marks a session free again (also on error/panic).
    struct Lease<'a> {
        model: &'a OrtModel,
        idx: usize,
    }

    impl Drop for Lease<'_> {
        fn drop(&mut self) {
            let mut b = self.model.busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(x) = b.get_mut(self.idx) {
                *x = false;
            }
            drop(b);
            self.model.cv.notify_all();
        }
    }

    impl OrtModel {
        fn lease(&self) -> Lease<'_> {
            let mut b = self.busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(i) = super::pick_session(&self.threads, &b) {
                    b[i] = true;
                    return Lease { model: self, idx: i };
                }
                b = self.cv.wait(b).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }

    impl Model for OrtModel {
        fn run(&self, input: Tensor) -> Result<Tensor> {
            let err = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", self.path.display()));
            let t = ort::value::Tensor::from_array((input.shape, input.data)).map_err(|e| err(&e))?;
            let lease = self.lease();
            let mut s = self.sessions[lease.idx].lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let out = s.run(ort::inputs![t]).map_err(|e| err(&e))?;
            if out.len() == 0 {
                return Err(Error::Model(format!("{}: model has no output", self.path.display())));
            }
            let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| err(&e))?;
            let shape: Vec<usize> = shape.iter().map(|&d| usize::try_from(d).unwrap_or(0)).collect();
            Tensor::new(shape, data.to_vec())
        }

        fn metadata(&self, key: &str) -> Option<String> {
            self.meta.get(key).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lone_run_takes_the_big_session_concurrent_ones_the_small() {
        let t = [6, 3, 3];
        assert_eq!(pick_session(&t, &[false, false, false]), Some(0));
        assert_eq!(pick_session(&t, &[true, false, false]), Some(1));
        assert_eq!(pick_session(&t, &[true, true, false]), Some(2));
        assert_eq!(pick_session(&t, &[true, true, true]), None);
        // big session free but others running: share (take a small one)
        assert_eq!(pick_session(&t, &[false, true, false]), Some(2));
        assert_eq!(pick_session(&t, &[false, true, true]), Some(0));
        assert_eq!(pick_session(&[4], &[false]), Some(0));
    }

    #[test]
    fn layouts() {
        assert_eq!(session_threads(6, 1), vec![6]);
        assert_eq!(session_threads(6, 3), vec![6, 3, 3]);
        assert_eq!(session_threads(6, 4), vec![6, 2, 2, 2]);
        assert_eq!(session_threads(2, 4), vec![2, 1, 1, 1]);
        assert_eq!(session_threads(0, 0), vec![1]);
    }
}
