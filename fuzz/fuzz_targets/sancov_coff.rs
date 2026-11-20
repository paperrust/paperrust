// Windows/MSVC: section markers for SanitizerCoverage come from the helper crate `sancov-coff`
// (own crate without functions so that LLVM does not rename the names). Link only.
#[cfg(all(windows, fuzzing))]
extern crate sancov_coff;
