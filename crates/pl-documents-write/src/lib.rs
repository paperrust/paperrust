//! pl-documents-write – write routes for documents. Contract: `docs/contract/pl-documents-write.md`.
//!
//! Implemented in Rust: PUT/PATCH/DELETE `/api/documents/{id}/` (incl. auditlog, "document
//! updated" workflows, file names/moving, search index), notes POST/DELETE, trash (list and
//! actions), `documents.bulk_edit` (all methods; the PDF methods via [`hooks::set_pdf_ops`]
//! with [`pdf_edit::NativePdfOps`], only with the PaperRust queue, otherwise Django) and
//! `documents.reprocess`. Worker tasks `documents.bulk_edit.delete`,
//! `restore_archive_serial_numbers_task`, `documents.tasks.empty_trash`: [`tasks`].

pub mod actions;
pub mod audit;
pub mod auditlog;
pub mod bulk_edit;
pub mod bulk_task;
pub mod cfi;
pub mod coalesce;
pub mod convert;
pub mod docs;
pub mod drf;
pub mod enqueue;
pub mod error;
pub mod files;
pub mod form;
pub mod hooks;
pub mod index;
pub mod notes;
pub mod pdf_edit;
pub mod pdftools;
pub mod perms;
pub mod redaction_marks;
pub mod selection;
pub mod serializer;
pub mod softdelete;
pub mod tasks;
pub mod trash;
pub mod trash_action;
pub mod update;
pub mod user;
pub mod versions;
pub mod views;
pub mod workflow_password;
pub mod workflows;

use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{delete, get, patch, post, put};
use pl_core::ApiRoute;
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_POST_ONLY, drf_view};

/// `Allow` of `TrashView` (GET, POST).
const ALLOW_TRASH: &str = "GET, POST, HEAD, OPTIONS";
/// `Allow` of the action `versions/<id>` (PATCH, DELETE).
const ALLOW_VERSIONS: &str = "PATCH, DELETE, OPTIONS";
/// `Allow` of the `notes` action (GET, POST, DELETE).
const ALLOW_NOTES: &str = "GET, POST, DELETE, HEAD, OPTIONS";

/// All routes of this module with a Rust handler. The server mounts only those enabled in
/// `routes.toml`; everything else goes to Django via proxy.
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new(
            "documents.update",
            Method::PUT,
            "/api/documents/{id}/",
            "src/documents/views.py::DocumentViewSet.update",
            drf_view(put(update::update_handler), ALLOW_MODEL_DETAIL),
        )
        // DRF default parsers (JSON, form, multipart): form bodies parsed in the handler
        .accepts_drf_parsers()
        .delegatable(),
        ApiRoute::new(
            "documents.partial_update",
            Method::PATCH,
            "/api/documents/{id}/",
            "src/documents/views.py::DocumentViewSet.partial_update",
            drf_view(patch(update::partial_update_handler), ALLOW_MODEL_DETAIL),
        )
        // DRF default parsers (JSON, form, multipart): form bodies parsed in the handler
        .accepts_drf_parsers()
        .delegatable(),
        ApiRoute::new(
            "documents.destroy",
            Method::DELETE,
            "/api/documents/{id}/",
            "src/documents/views.py::DocumentViewSet.destroy",
            drf_view(delete(update::destroy_handler), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "documents.notes.create",
            Method::POST,
            "/api/documents/{id}/notes/",
            "src/documents/views.py::DocumentViewSet.notes (POST)",
            drf_view(post(notes::notes_create_handler), ALLOW_NOTES),
        )
        // DRF default parsers (JSON, form, multipart): form bodies parsed in the handler
        .accepts_drf_parsers()
        .delegatable(),
        ApiRoute::new(
            "documents.notes.destroy",
            Method::DELETE,
            "/api/documents/{id}/notes/",
            "src/documents/views.py::DocumentViewSet.notes (DELETE)",
            drf_view(delete(notes::notes_destroy_handler), ALLOW_NOTES),
        ),
        ApiRoute::new(
            "documents.bulk_edit",
            Method::POST,
            "/api/documents/bulk_edit/",
            "src/documents/views.py::BulkEditView.post",
            drf_view(post(views::bulk_edit_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.reprocess",
            Method::POST,
            "/api/documents/reprocess/",
            "src/documents/views.py::ReprocessDocumentsView.post",
            drf_view(post(views::reprocess_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "trash.list",
            Method::GET,
            "/api/trash/",
            "src/documents/views.py::TrashView.get",
            drf_view(get(trash::trash_list_handler), ALLOW_TRASH),
        ),
        ApiRoute::new(
            "trash.action",
            Method::POST,
            "/api/trash/",
            "src/documents/views.py::TrashView.post",
            drf_view(post(trash_action::trash_action_handler), ALLOW_TRASH),
        )
        // DRF default parsers (JSON, form, multipart): form bodies parsed in the handler
        .accepts_drf_parsers()
        .delegatable(),
        // W23: own action routes (instead of bulk_edit methods), versions
        ApiRoute::new(
            "documents.delete",
            Method::POST,
            "/api/documents/delete/",
            "src/documents/views.py::DeleteDocumentsView.post",
            drf_view(post(actions::delete_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.rotate",
            Method::POST,
            "/api/documents/rotate/",
            "src/documents/views.py::RotateDocumentsView.post",
            drf_view(post(actions::rotate_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.merge",
            Method::POST,
            "/api/documents/merge/",
            "src/documents/views.py::MergeDocumentsView.post",
            drf_view(post(actions::merge_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.merge_as_versions",
            Method::POST,
            "/api/documents/merge_as_versions/",
            "src/documents/views.py::MergeDocumentsAsVersionsView.post",
            drf_view(post(actions::merge_as_versions_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.edit_pdf",
            Method::POST,
            "/api/documents/edit_pdf/",
            "src/documents/views.py::EditPdfDocumentsView.post",
            drf_view(post(actions::edit_pdf_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.remove_password",
            Method::POST,
            "/api/documents/remove_password/",
            "src/documents/views.py::RemovePasswordDocumentsView.post",
            drf_view(post(actions::remove_password_handler), ALLOW_POST_ONLY),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.versions.destroy",
            Method::DELETE,
            "/api/documents/{id}/versions/{version_id}/",
            "src/documents/views.py::DocumentViewSet.delete_version",
            drf_view(delete(versions::delete_version_handler), ALLOW_VERSIONS),
        )
        .delegatable(),
        ApiRoute::new(
            "documents.versions.partial_update",
            Method::PATCH,
            "/api/documents/{id}/versions/{version_id}/",
            "src/documents/views.py::DocumentViewSet.update_version_label",
            drf_view(patch(versions::update_version_label_handler), ALLOW_VERSIONS),
        )
        // DRF default parsers (JSON, form, multipart): form bodies parsed in the handler
        .accepts_drf_parsers()
        .delegatable(),
        ApiRoute::new(
            "documents.update_version",
            Method::POST,
            "/api/documents/{id}/update_version/",
            "src/documents/views.py::DocumentViewSet.update_version",
            drf_view(post(versions::update_version_handler).layer(DefaultBodyLimit::disable()), ALLOW_POST_ONLY),
        )
        // parser_classes=[MultiPartParser]: multipart here, JSON/form → 415 here
        .accepts_drf_parsers()
        .delegatable(),
        // PaperRust-specific: PDF tools (pl-pdftools, docs/contract/pl-pdftools.md §4)
        ApiRoute::new(
            "documents.redact_search",
            Method::POST,
            "/api/documents/{id}/redact/search/",
            "PaperRust (pl-pdftools): Suchbegriffe für das Schwärzen finden",
            drf_view(post(pdftools::redact_search_handler), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "documents.redact",
            Method::POST,
            "/api/documents/{id}/redact/",
            "PaperRust (pl-pdftools): Schwärzen als neue Version",
            drf_view(post(pdftools::redact_handler), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "documents.shrink",
            Method::POST,
            "/api/documents/{id}/shrink/",
            "PaperRust (pl-pdftools): Verkleinern als neue Version",
            drf_view(post(pdftools::shrink_handler), ALLOW_POST_ONLY),
        ),
        // PaperRust-specific: convert to PDF (images, Office), docs/contract/pl-pdftools.md §7
        ApiRoute::new(
            "documents.convert_pdf",
            Method::POST,
            "/api/documents/{id}/convert_pdf/",
            "PaperRust (pl-pdftools): Bild/Office in PDF umwandeln als neue Version",
            drf_view(post(convert::convert_pdf_handler), ALLOW_POST_ONLY),
        ),
    ]
}
