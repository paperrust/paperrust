//! Port selection: only `127.0.0.1`, preferring the most recently used port (stable URL → bookmarks and
//! session cookie stay valid), otherwise a free port from the operating system.
//!
//! "Free" means: nobody listens on `127.0.0.1:p` (connection attempt fails) **and** we can bind
//! there. The connection attempt catches the Windows case where another process listens on
//! `0.0.0.0:p` without `SO_EXCLUSIVEADDRUSE` and binding to `127.0.0.1:p` still succeeds.
//! A small race (TOCTOU) remains between the check and the start by PostgreSQL/the server – the
//! supervisor therefore tries a new port once on a startup failure.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

pub const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Default preferred ports on first start (rarely in use; not 5432, so an existing
/// PostgreSQL of the user does not collide, not 8000/8080/8100/8101).
pub const DEFAULT_DB_PORT: u16 = 54_417;
pub const DEFAULT_APP_PORT: u16 = 58_080;

/// Never choose: common services and the ports of the project's own development/parity environments.
/// 54329/63799/8001: reference environment of this project (reference/README.md).
pub const AVOID: &[u16] = &[5432, 8000, 8001, 8080, 8100, 8101, 54_329, 63_799];

pub fn addr(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(LOOPBACK, port))
}

/// Is anyone listening on `127.0.0.1:port`?
pub fn is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(&addr(port), Duration::from_millis(200)).is_ok()
}

/// Is `127.0.0.1:port` free (nobody listens, binding succeeds)?
pub fn is_free(port: u16) -> bool {
    port != 0 && !is_listening(port) && TcpListener::bind(addr(port)).is_ok()
}

/// Picks a port: `preferred` if free and not in `avoid`, otherwise a free one from the OS.
pub fn pick_port(preferred: Option<u16>, avoid: &[u16]) -> std::io::Result<u16> {
    if let Some(p) = preferred.filter(|p| !avoid.contains(p) && !AVOID.contains(p))
        && is_free(p)
    {
        return Ok(p);
    }
    for _ in 0..32 {
        let l = TcpListener::bind(addr(0))?;
        let p = l.local_addr()?.port();
        drop(l);
        if !avoid.contains(&p) && !AVOID.contains(&p) && is_free(p) {
            return Ok(p);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "kein freier Port auf 127.0.0.1 gefunden",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_when_free() {
        let p = pick_port(None, &[]).unwrap();
        assert_eq!(pick_port(Some(p), &[]).unwrap(), p);
    }

    #[test]
    fn busy_preferred_is_replaced() {
        let l = TcpListener::bind(addr(0)).unwrap();
        let busy = l.local_addr().unwrap().port();
        assert!(is_listening(busy));
        assert!(!is_free(busy));
        let p = pick_port(Some(busy), &[]).unwrap();
        assert_ne!(p, busy);
        assert!(is_free(p));
    }

    #[test]
    fn avoid_list_and_zero() {
        let free = pick_port(None, &[]).unwrap();
        let p = pick_port(Some(free), &[free]).unwrap();
        assert_ne!(p, free);
        assert_ne!(pick_port(Some(8100), &[]).unwrap(), 8100);
        assert_ne!(pick_port(Some(0), &[]).unwrap(), 0);
        assert!(!is_free(0));
    }
}
