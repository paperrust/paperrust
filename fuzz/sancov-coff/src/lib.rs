//! Data only, no functions (otherwise LLVM instruments this crate too and renames the markers).
#![no_std]
// Windows/MSVC: start/end markers of the SanitizerCoverage sections. LLVM expects them on COFF from
// compiler-rt (sanitizer_coverage_win_sections.cpp), libfuzzer-sys does not bring them along -> otherwise
// LNK2001 "__start___sancov_cntrs". The MSVC linker sorts ".SCOV$CA" < ".SCOV$CM" (LLVM counters)
// < ".SCOV$CZ"; on COFF LLVM discounts the 8 bytes of the start marker itself.
#[cfg(windows)]
#[allow(non_upper_case_globals, dead_code)]
pub mod markers {
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOV$CA")]
    pub static mut __start___sancov_cntrs: u64 = 0;
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOV$CZ")]
    pub static mut __stop___sancov_cntrs: u8 = 0;
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOVP$A")]
    pub static __start___sancov_pcs: u64 = 0;
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOVP$Z")]
    pub static __stop___sancov_pcs: u8 = 0;
}
