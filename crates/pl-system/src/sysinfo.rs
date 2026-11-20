//! Host information for `/api/status/` computed like the Python standard library.
//!
//! upstream: src/documents/views.py@v3.4.2 (`SystemStatusView.get`): `platform.platform()`,
//! `os.statvfs(MEDIA_ROOT)`, `urlparse(settings._CHANNELS_REDIS_URL)`,
//! `datetime.fromtimestamp(max(mtimes))` + `make_aware`.

use std::path::Path;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use chrono_tz::Tz;

/// `install_type` from the environment.
pub fn install_type(kubernetes_host: Option<&str>, containerized: Option<&str>) -> &'static str {
    if kubernetes_host.is_some() {
        "kubernetes"
    } else if containerized == Some("1") {
        "docker"
    } else {
        "bare-metal"
    }
}

/// `platform.platform()` (Linux: `Linux-<release>-<machine>-with-glibc<ver>`,
/// Windows: `Windows-<release>-<version>-<csd>`, e.g. `Windows-11-10.0.22631-SP0`).
pub fn server_os() -> String {
    #[cfg(unix)]
    {
        unix_platform()
    }
    #[cfg(windows)]
    {
        windows_platform()
    }
    #[cfg(not(any(unix, windows)))]
    {
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// CPython 3.12 `platform._WIN32_CLIENT_RELEASES` / `_WIN32_SERVER_RELEASES`.
const WIN32_CLIENT_RELEASES: &[((u32, u32, u32), &str)] = &[
    ((10, 1, 0), "post11"),
    ((10, 0, 22000), "11"),
    ((6, 4, 0), "10"),
    ((6, 3, 0), "8.1"),
    ((6, 2, 0), "8"),
    ((6, 1, 0), "7"),
    ((6, 0, 0), "Vista"),
    ((5, 2, 3790), "XP64"),
    ((5, 2, 0), "XPMedia"),
    ((5, 1, 0), "XP"),
    ((5, 0, 0), "2000"),
];
const WIN32_SERVER_RELEASES: &[((u32, u32, u32), &str)] = &[
    ((10, 1, 0), "post2025Server"),
    ((10, 0, 26100), "2025Server"),
    ((10, 0, 20348), "2022Server"),
    ((10, 0, 17763), "2019Server"),
    ((6, 4, 0), "2016Server"),
    ((6, 3, 0), "2012ServerR2"),
    ((6, 2, 0), "2012Server"),
    ((6, 1, 0), "2008ServerR2"),
    ((6, 0, 0), "2008Server"),
    ((5, 2, 0), "2003Server"),
    ((5, 0, 0), "2000Server"),
];

/// `platform.platform()` on Windows from the values CPython's `_win32_ver` reads via WMI
/// (`Win32_OperatingSystem`: Version, ProductType, ServicePackMajor/MinorVersion):
/// `_platform("Windows", release, version, csd)` with `release` from `win32_ver`'s table.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_platform_from(major: u32, minor: u32, build: u32, product_type: u8, sp_major: u16, sp_minor: u16) -> String {
    let version = format!("{major}.{minor}.{build}");
    let csd = if sp_minor != 0 { format!("SP{sp_major}.{sp_minor}") } else { format!("SP{sp_major}") };
    let releases = if product_type == 1 { WIN32_CLIENT_RELEASES } else { WIN32_SERVER_RELEASES };
    let release = releases.iter().find(|(v, _)| *v <= (major, minor, build)).map(|(_, r)| *r).unwrap_or("");
    platform_join(&["Windows".into(), release.into(), version, csd])
}

#[cfg(windows)]
#[repr(C)]
struct OsVersionInfoExW {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
    w_service_pack_major: u16,
    w_service_pack_minor: u16,
    w_suite_mask: u16,
    w_product_type: u8,
    w_reserved: u8,
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    /// ntdll `RtlGetVersion` (the real version, not the manifest-dependent `GetVersionEx`).
    fn RtlGetVersion(info: *mut OsVersionInfoExW) -> i32;
}

#[cfg(windows)]
fn windows_platform() -> String {
    // SAFETY: all-zero is a valid OSVERSIONINFOEXW; the size field is set before the call.
    let mut info: OsVersionInfoExW = unsafe { std::mem::zeroed() };
    info.dw_os_version_info_size = std::mem::size_of::<OsVersionInfoExW>() as u32;
    // SAFETY: valid pointer to a properly sized, initialized struct; nothing is retained.
    let rc = unsafe { RtlGetVersion(&mut info) };
    if rc != 0 {
        return format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    }
    windows_platform_from(
        info.dw_major_version,
        info.dw_minor_version,
        info.dw_build_number,
        info.w_product_type,
        info.w_service_pack_major,
        info.w_service_pack_minor,
    )
}

#[cfg(unix)]
fn cstr_field(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(unix)]
fn unix_platform() -> String {
    // SAFETY: `uname` fills a zeroed, properly sized struct; no pointers are retained.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointer to a local utsname.
    let rc = unsafe { libc::uname(&mut u) };
    if rc != 0 {
        return String::new();
    }
    let system = cstr_field(&u.sysname);
    let release = cstr_field(&u.release);
    let machine = cstr_field(&u.machine);
    let mut parts = vec![system.clone(), release, machine];
    if system == "Linux"
        && let Some((name, ver)) = glibc_version()
    {
        parts.push("with".into());
        parts.push(format!("{name}{ver}"));
    }
    platform_join(&parts)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn glibc_version() -> Option<(String, String)> {
    let mut buf = vec![0 as libc::c_char; 256];
    // SAFETY: buffer and length are valid; confstr writes at most len bytes.
    let n = unsafe { libc::confstr(libc::_CS_GNU_LIBC_VERSION, buf.as_mut_ptr(), buf.len()) };
    if n == 0 {
        return None;
    }
    let s = cstr_field(&buf);
    let (a, b) = s.split_once(char::is_whitespace)?;
    Some((a.to_string(), b.trim().to_string()))
}

#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
fn glibc_version() -> Option<(String, String)> {
    None
}

/// `platform._platform(*args)`: join non-empty parts with `-`, clean special characters.
fn platform_join(parts: &[String]) -> String {
    let mut p = parts.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("-");
    p = p.replace(' ', "_");
    p = p.replace(['/', '\\', ':', ';', '"', '(', ')'], "-");
    p = p.replace("unknown", "");
    loop {
        let c = p.replace("--", "-");
        if c == p {
            break;
        }
        p = c;
    }
    p.trim_end_matches('-').to_string()
}

/// `(f_frsize * f_blocks, f_frsize * f_bavail)` of `statvfs(path)`.
pub fn storage(path: &Path) -> Result<(u64, u64), String> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: zeroed struct is a valid out-parameter for statvfs.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is a valid NUL-terminated path; s is a valid out pointer.
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        #[allow(clippy::unnecessary_cast)]
        let frsize = s.f_frsize as u64;
        #[allow(clippy::unnecessary_cast)]
        Ok((frsize * s.f_blocks as u64, frsize * s.f_bavail as u64))
    }
    #[cfg(windows)]
    {
        // upstream has no os.statvfs on Windows (→ 500); the reference environment shims it with
        // shutil.disk_usage (f_frsize=1, f_blocks=total, f_bavail=free) – mirrored here so the
        // Windows reference can be compared. Production (Linux) uses statvfs above.
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: NUL-terminated wide string and valid out pointers.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free)
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok((total, free))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err("os.statvfs is not available on this platform".into())
    }
}

/// `datetime.fromtimestamp(ts)` for a float like `os.stat().st_mtime` (round-half-even to
/// microseconds), as naive local time.
pub fn fromtimestamp_local(mtime_ns: i128) -> Option<NaiveDateTime> {
    // Python: st_mtime = sec + nsec * 1e-9 as a C double
    let secs = mtime_ns.div_euclid(1_000_000_000);
    let nsec = mtime_ns.rem_euclid(1_000_000_000);
    let t = secs as f64 + nsec as f64 * 1e-9;
    // datetime._fromtimestamp: frac, t = modf(t); us = round(frac * 1e6) (half-even)
    let int_part = t.trunc();
    let frac = t - int_part;
    let us_f = frac * 1e6;
    let mut us = round_half_even(us_f) as i64;
    let mut s = int_part as i64;
    if us >= 1_000_000 {
        s += 1;
        us -= 1_000_000;
    } else if us < 0 {
        s -= 1;
        us += 1_000_000;
    }
    let utc = DateTime::from_timestamp(s, (us * 1000) as u32)?;
    Some(utc.with_timezone(&Local).naive_local())
}

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        r
    }
}

/// `make_aware(naive)` in the Django current time zone (`TIME_ZONE`), `is_dst=None` semantics
/// of zoneinfo: ambiguous → first (fold=0), non-existent → shifted like zoneinfo (fold=0 uses
/// the offset before the gap).
pub fn make_aware(naive: NaiveDateTime, tz: Tz) -> DateTime<Tz> {
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(d) => d,
        chrono::LocalResult::Ambiguous(a, _) => a,
        chrono::LocalResult::None => {
            // zoneinfo with fold=0 interprets the wall time with the offset before the gap
            let before = naive - chrono::Duration::hours(3);
            let off = tz.offset_from_local_datetime(&before).earliest();
            match off {
                Some(o) => {
                    use chrono::Offset;
                    let utc = naive - chrono::Duration::seconds(i64::from(o.fix().local_minus_utc()));
                    tz.from_utc_datetime(&utc)
                }
                None => tz.from_utc_datetime(&naive),
            }
        }
    }
}

/// Most recent mtime (ns) of the regular files directly in `dir` (`p.is_file()` follows links).
/// `Err` if the directory cannot be listed.
pub fn newest_file_mtime_ns(dir: &Path) -> std::io::Result<Option<i128>> {
    let mut best: Option<i128> = None;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        let m = meta.modified()?;
        let ns = match m.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        };
        best = Some(best.map_or(ns, |b| b.max(ns)));
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_types() {
        assert_eq!(install_type(None, None), "bare-metal");
        assert_eq!(install_type(None, Some("1")), "docker");
        assert_eq!(install_type(Some("x"), Some("1")), "kubernetes");
    }

    #[test]
    fn windows_platform_like_cpython() {
        assert_eq!(windows_platform_from(10, 0, 22631, 1, 0, 0), "Windows-11-10.0.22631-SP0");
        assert_eq!(windows_platform_from(10, 0, 19045, 1, 0, 0), "Windows-10-10.0.19045-SP0");
        assert_eq!(windows_platform_from(10, 0, 20348, 3, 0, 0), "Windows-2022Server-10.0.20348-SP0");
        assert_eq!(windows_platform_from(6, 1, 7601, 1, 1, 0), "Windows-7-6.1.7601-SP1");
        assert_eq!(windows_platform_from(6, 1, 7601, 1, 1, 2), "Windows-7-6.1.7601-SP1.2");
    }

    #[cfg(windows)]
    #[test]
    fn windows_platform_shape() {
        let p = server_os();
        assert!(p.starts_with("Windows-"), "{p}");
        assert!(p.contains("-SP"), "{p}");
    }

    #[test]
    fn platform_join_cleans() {
        let p = platform_join(&["Linux".into(), "6.8.0-45-generic".into(), "x86_64".into(), "with".into(), "glibc2.36".into()]);
        assert_eq!(p, "Linux-6.8.0-45-generic-x86_64-with-glibc2.36");
    }

    #[test]
    fn rounding() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.4), 2.0);
    }
}
