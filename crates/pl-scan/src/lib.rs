//! pl-scan – connect scanners without NAS access on the device (contract: docs/contract/pl-scan.md).
//!
//! Two paths, both hand a file to the normal ingestion pipeline (`consume_file`):
//!
//! 1. **Scan button** in PaperRust via eSCL/AirScan ([`escl`], [`job`], discovery: [`mdns`]).
//!    The result belongs to the user who pressed the button (source "Web UI").
//! 2. **Device email** ([`smtp`]): small SMTP intake, off until `PAPERRUST_SMTP_INTAKE=on`.
//!    Every device has its own token ([`token`]) that can only upload.
//!
//! | Route id | Method | Path | Permission |
//! |---|---|---|---|
//! | `scan.scanners.list` | GET | `/api/x-scan/scanners/` | `add_document` |
//! | `scan.scanners.create` | POST | `/api/x-scan/scanners/` | Superuser |
//! | `scan.scanners.update` | PATCH | `/api/x-scan/scanners/{id}/` | Superuser |
//! | `scan.scanners.delete` | DELETE | `/api/x-scan/scanners/{id}/` | Superuser |
//! | `scan.scanners.capabilities` | GET | `/api/x-scan/scanners/{id}/capabilities/` | `add_document` |
//! | `scan.discover` | GET | `/api/x-scan/discover/` | Superuser |
//! | `scan.jobs.create` | POST | `/api/x-scan/jobs/` | `add_document` |
//! | `scan.jobs.retrieve` | GET | `/api/x-scan/jobs/{id}/` | own job |
//! | `scan.jobs.cancel` | POST | `/api/x-scan/jobs/{id}/cancel/` | own job |
//! | `scan.devices.list` | GET | `/api/x-scan/devices/` | Superuser |
//! | `scan.devices.create` | POST | `/api/x-scan/devices/` | Superuser |
//! | `scan.devices.revoke` | POST | `/api/x-scan/devices/{id}/revoke/` | Superuser |
//! | `scan.devices.delete` | DELETE | `/api/x-scan/devices/{id}/` | Superuser |

pub mod api;
pub mod escl;
pub mod intake;
pub mod job;
pub mod mdns;
pub mod net;
pub mod pdf;
pub mod smtp;
pub mod store;
pub mod token;
pub mod xml;

use axum::http::Method;
use axum::routing::{delete, get, patch, post};
use pl_core::ApiRoute;
use pl_core::route::drf_view;

const SRC: &str = "pl-scan extension (scanner intake)";
const ALLOW_GET: &str = "GET, HEAD, OPTIONS";
const ALLOW_POST: &str = "POST, OPTIONS";
const ALLOW_LIST: &str = "GET, POST, HEAD, OPTIONS";
const ALLOW_DETAIL: &str = "PATCH, DELETE, OPTIONS";

/// All routes (ids in routes.toml).
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new("scan.scanners.list", Method::GET, "/api/x-scan/scanners/", SRC, drf_view(get(api::scanners_list), ALLOW_LIST)),
        ApiRoute::new("scan.scanners.create", Method::POST, "/api/x-scan/scanners/", SRC, drf_view(post(api::scanners_create), ALLOW_LIST)),
        ApiRoute::new(
            "scan.scanners.update",
            Method::PATCH,
            "/api/x-scan/scanners/{id}/",
            SRC,
            drf_view(patch(api::scanners_update), ALLOW_DETAIL),
        ),
        ApiRoute::new(
            "scan.scanners.delete",
            Method::DELETE,
            "/api/x-scan/scanners/{id}/",
            SRC,
            drf_view(delete(api::scanners_delete), ALLOW_DETAIL),
        ),
        ApiRoute::new(
            "scan.scanners.capabilities",
            Method::GET,
            "/api/x-scan/scanners/{id}/capabilities/",
            SRC,
            drf_view(get(api::scanners_capabilities), ALLOW_GET),
        ),
        ApiRoute::new("scan.discover", Method::GET, "/api/x-scan/discover/", SRC, drf_view(get(api::discover), ALLOW_GET)),
        ApiRoute::new("scan.jobs.create", Method::POST, "/api/x-scan/jobs/", SRC, drf_view(post(api::jobs_create), ALLOW_POST)),
        ApiRoute::new("scan.jobs.retrieve", Method::GET, "/api/x-scan/jobs/{id}/", SRC, drf_view(get(api::jobs_retrieve), ALLOW_GET)),
        ApiRoute::new("scan.jobs.cancel", Method::POST, "/api/x-scan/jobs/{id}/cancel/", SRC, drf_view(post(api::jobs_cancel), ALLOW_POST)),
        ApiRoute::new("scan.devices.list", Method::GET, "/api/x-scan/devices/", SRC, drf_view(get(api::devices_list), ALLOW_LIST)),
        ApiRoute::new("scan.devices.create", Method::POST, "/api/x-scan/devices/", SRC, drf_view(post(api::devices_create), ALLOW_LIST)),
        ApiRoute::new(
            "scan.devices.revoke",
            Method::POST,
            "/api/x-scan/devices/{id}/revoke/",
            SRC,
            drf_view(post(api::devices_revoke), ALLOW_POST),
        ),
        ApiRoute::new(
            "scan.devices.delete",
            Method::DELETE,
            "/api/x-scan/devices/{id}/",
            SRC,
            drf_view(delete(api::devices_delete), "DELETE, OPTIONS"),
        ),
    ]
}

/// Route ids (for routes.toml and tests).
pub fn route_ids() -> Vec<&'static str> {
    routes().iter().map(|r| r.id).collect()
}
