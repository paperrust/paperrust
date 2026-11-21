//! Matching parts of upstream src/documents/tests/test_workflows.py@v3.4.2 without DB:
//! the trigger checks and their reason texts (which upstream asserts via the
//! "paperless.matching" log output).

use pl_matching::matching::{MATCH_ANY, MATCH_LITERAL};
use pl_matching::workflow::*;

fn c() -> ObjRef {
    ObjRef::new(1, "Correspondent Name")
}
fn c2() -> ObjRef {
    ObjRef::new(2, "Correspondent Name 2")
}
fn dt() -> ObjRef {
    ObjRef::new(1, "DocType Name")
}
fn sp() -> ObjRef {
    ObjRef::new(1, "Storage Path 1")
}
fn t(id: i64) -> ObjRef {
    ObjRef::new(id, &format!("t{id}"))
}

fn consumable(source: DocumentSource, file: &str) -> ConsumableDocument {
    ConsumableDocument { source, original_file: file.into(), original_path: None, mailrule_id: None }
}

fn sources_all() -> Vec<i64> {
    vec![2, 1, 3]
}

const SCRATCH: &str = "/tmp/paperless-tests/scratch/simple.pdf";

#[test]
fn test_workflow_match() {
    let trig = WorkflowTrigger {
        id: 1,
        trigger_type: TRIGGER_CONSUMPTION,
        sources: sources_all(),
        filter_filename: Some("*simple*".into()),
        filter_path: Some("*/scratch/*".into()),
        ..Default::default()
    };
    let d = consumable(DocumentSource::ConsumeFolder, SCRATCH);
    assert_eq!(consumable_document_matches_workflow(&d, &trig), (true, String::new()));
    assert!(document_matches_workflow(WorkflowDocument::Consumable(&d), "Workflow 1", &[trig], TRIGGER_CONSUMPTION, None).unwrap());
}

#[test]
fn test_workflow_match_mailrule() {
    let trig =
        WorkflowTrigger { trigger_type: TRIGGER_CONSUMPTION, sources: sources_all(), filter_mailrule_id: Some(7), ..Default::default() };
    let mut d = consumable(DocumentSource::MailFetch, SCRATCH);
    d.mailrule_id = Some(7);
    assert!(consumable_document_matches_workflow(&d, &trig).0);
}

#[test]
fn test_workflow_fnmatch_path() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_CONSUMPTION,
        sources: sources_all(),
        filter_path: Some("*scratch*".into()),
        ..Default::default()
    };
    assert!(consumable_document_matches_workflow(&consumable(DocumentSource::ConsumeFolder, SCRATCH), &trig).0);
}

#[test]
fn test_workflow_no_match_filename() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_CONSUMPTION,
        sources: sources_all(),
        filter_filename: Some("*foobar*".into()),
        filter_path: None,
        ..Default::default()
    };
    let (m, reason) = consumable_document_matches_workflow(&consumable(DocumentSource::ConsumeFolder, SCRATCH), &trig);
    assert!(!m);
    assert!(reason.starts_with("Document filename simple.pdf does not match"), "{reason}");
}

#[test]
fn test_workflow_no_match_path() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_CONSUMPTION,
        sources: sources_all(),
        filter_path: Some("*foo/bar*".into()),
        ..Default::default()
    };
    let (m, reason) = consumable_document_matches_workflow(&consumable(DocumentSource::ConsumeFolder, SCRATCH), &trig);
    assert!(!m);
    assert!(reason.starts_with(&format!("Document path {SCRATCH} does not match")), "{reason}");
}

#[test]
fn test_workflow_no_match_mail_rule() {
    let trig =
        WorkflowTrigger { trigger_type: TRIGGER_CONSUMPTION, sources: sources_all(), filter_mailrule_id: Some(1), ..Default::default() };
    let mut d = consumable(DocumentSource::ConsumeFolder, SCRATCH);
    d.mailrule_id = Some(99);
    let (m, reason) = consumable_document_matches_workflow(&d, &trig);
    assert!(!m);
    assert!(reason.contains("Document mail rule 99 !="), "{reason}");
}

#[test]
fn test_workflow_no_match_source() {
    let trig =
        WorkflowTrigger { trigger_type: TRIGGER_CONSUMPTION, sources: vec![1, 3], filter_path: Some("*".into()), ..Default::default() };
    let (m, reason) = consumable_document_matches_workflow(&consumable(DocumentSource::ApiUpload, SCRATCH), &trig);
    assert!(!m);
    assert_eq!(reason, "Document source ApiUpload not in ['ConsumeFolder', 'MailFetch']");
}

#[test]
fn test_workflow_filename_is_case_insensitive_and_original_path_used() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_CONSUMPTION,
        filter_filename: Some("*SIMPLE*".into()),
        filter_path: Some("/orig/*".into()),
        ..Default::default()
    };
    let mut d = consumable(DocumentSource::ConsumeFolder, "/tmp/x/Simple.PDF");
    d.original_path = Some("/orig/sub/Simple.PDF".into());
    assert!(consumable_document_matches_workflow(&d, &trig).0);
}

#[test]
fn test_document_added_no_match_trigger_type() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_CONSUMPTION, ..Default::default() };
    let d = ExistingDocument::default();
    assert!(!document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[trig], TRIGGER_DOCUMENT_ADDED, None).unwrap());
}

fn sample_doc() -> ExistingDocument {
    ExistingDocument {
        id: 1,
        effective_content: Some("Hello world".into()),
        display: "2024-01-01 sample test".into(),
        tags: vec![t(3)],
        correspondent: Some(c()),
        document_type: None,
        storage_path: None,
        original_filename: Some("sample.pdf".into()),
    }
}

#[test]
fn test_document_added_no_match_filename() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, filter_filename: Some("*foobar*".into()), ..Default::default() };
    let (m, reason) = existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document filename sample.pdf does not match *foobar*");
}

#[test]
fn test_document_added_match_content_matching() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_DOCUMENT_ADDED,
        matching_algorithm: MATCH_LITERAL,
        match_text: "world".into(),
        is_insensitive: true,
        ..Default::default()
    };
    assert_eq!(existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap(), (true, None));
}

#[test]
fn test_document_added_no_match_content_matching() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_DOCUMENT_ADDED,
        matching_algorithm: MATCH_LITERAL,
        match_text: "foobar".into(),
        is_insensitive: true,
        ..Default::default()
    };
    let (m, reason) = existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document content matching settings for algorithm '3' did not match");
}

#[test]
fn test_document_added_no_match_tags() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, filter_has_tags: vec![t(1), t(2)], ..Default::default() };
    let (m, reason) = existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document tags [<Tag: t3>] do not include [<Tag: t1>, <Tag: t2>]");
}

#[test]
fn test_document_added_no_match_all_tags() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, filter_has_all_tags: vec![t(1), t(2)], ..Default::default() };
    let mut d = sample_doc();
    d.tags = vec![t(1), t(3)];
    let (m, reason) = existing_document_matches_workflow(&d, &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document tags [<Tag: t1>, <Tag: t3>] do not contain all of [<Tag: t1>, <Tag: t2>]");
}

#[test]
fn test_document_added_excluded_tags() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, filter_has_not_tags: vec![t(3)], ..Default::default() };
    let (m, reason) = existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document tags [<Tag: t3>] include excluded tags [<Tag: t3>]");
}

#[test]
fn test_document_added_excluded_correspondent() {
    let trig = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, filter_has_not_correspondents: vec![c()], ..Default::default() };
    let (m, reason) = existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap();
    assert!(!m);
    assert_eq!(reason.unwrap(), "Document correspondent Correspondent Name is excluded by [<Correspondent: Correspondent Name>]");
}

#[test]
fn test_document_added_excluded_document_types_and_storage_paths() {
    let mut d = sample_doc();
    d.document_type = Some(dt());
    d.storage_path = Some(sp());
    let trig = WorkflowTrigger { filter_has_not_document_types: vec![dt()], ..Default::default() };
    let (m, r) = existing_document_matches_workflow(&d, &trig, None).unwrap();
    assert!(!m);
    assert_eq!(r.unwrap(), "Document doc type DocType Name is excluded by [<DocumentType: DocType Name>]");
    let trig = WorkflowTrigger { filter_has_not_storage_paths: vec![sp()], ..Default::default() };
    let (m, r) = existing_document_matches_workflow(&d, &trig, None).unwrap();
    assert!(!m);
    assert_eq!(r.unwrap(), "Document storage path Storage Path 1 is excluded by [<StoragePath: Storage Path 1>]");
}

#[test]
fn test_document_added_any_filters() {
    let trig = WorkflowTrigger {
        trigger_type: TRIGGER_DOCUMENT_ADDED,
        filter_has_any_correspondents: vec![c()],
        filter_has_any_document_types: vec![dt()],
        filter_has_any_storage_paths: vec![sp()],
        ..Default::default()
    };
    let mut ok = sample_doc();
    ok.document_type = Some(dt());
    ok.storage_path = Some(sp());
    assert_eq!(existing_document_matches_workflow(&ok, &trig, None).unwrap(), (true, None));

    let mut wrong = ok.clone();
    wrong.correspondent = Some(c2());
    let (m, r) = existing_document_matches_workflow(&wrong, &trig, None).unwrap();
    assert!(!m && r.unwrap().contains("correspondent"));

    let mut wrong = ok.clone();
    wrong.document_type = Some(ObjRef::new(9, "Other"));
    let (m, r) = existing_document_matches_workflow(&wrong, &trig, None).unwrap();
    assert!(!m && r.unwrap().contains("doc type"));

    let mut wrong = ok.clone();
    wrong.storage_path = Some(ObjRef::new(9, "Other path"));
    let (m, r) = existing_document_matches_workflow(&wrong, &trig, None).unwrap();
    assert!(!m && r.unwrap().contains("storage path"));
}

#[test]
fn test_document_added_no_match_doctype_correspondent_storage_path() {
    let d = sample_doc();
    let trig = WorkflowTrigger { filter_has_document_type: Some(dt()), ..Default::default() };
    assert_eq!(
        existing_document_matches_workflow(&d, &trig, None).unwrap().1.unwrap(),
        "Document doc type None does not match DocType Name"
    );
    let trig = WorkflowTrigger { filter_has_correspondent: Some(c2()), ..Default::default() };
    assert_eq!(
        existing_document_matches_workflow(&d, &trig, None).unwrap().1.unwrap(),
        "Document correspondent Correspondent Name does not match Correspondent Name 2"
    );
    let trig = WorkflowTrigger { filter_has_storage_path: Some(sp()), ..Default::default() };
    assert_eq!(
        existing_document_matches_workflow(&d, &trig, None).unwrap().1.unwrap(),
        "Document storage path None does not match Storage Path 1"
    );
}

struct Cf(Result<bool, InvalidCustomFieldQuery>);

impl CustomFieldQueryEvaluator for Cf {
    fn matches(&self, _query: &str, _document_id: i64) -> Result<bool, InvalidCustomFieldQuery> {
        self.0.clone()
    }
}

#[test]
fn test_document_added_custom_field_query() {
    let trig = WorkflowTrigger { filter_custom_field_query: Some(r#"["AND", [[1, "exact", "x"]]]"#.into()), ..Default::default() };
    let d = sample_doc();
    assert_eq!(existing_document_matches_workflow(&d, &trig, Some(&Cf(Ok(true)))).unwrap(), (true, None));
    assert_eq!(
        existing_document_matches_workflow(&d, &trig, Some(&Cf(Ok(false)))).unwrap().1.unwrap(),
        "Document custom fields do not match the configured custom field query"
    );
    assert_eq!(
        existing_document_matches_workflow(&d, &trig, Some(&Cf(Err(InvalidCustomFieldQuery)))).unwrap().1.unwrap(),
        "Invalid custom field query configuration"
    );
    assert!(prefilter_matches(&d, &trig, Some(&Cf(Ok(true)))));
    assert!(!prefilter_matches(&d, &trig, Some(&Cf(Err(InvalidCustomFieldQuery)))));
}

#[test]
fn test_prefilter_documents_any_filters() {
    let trig = WorkflowTrigger {
        filter_has_any_correspondents: vec![c()],
        filter_has_any_document_types: vec![dt()],
        filter_has_any_storage_paths: vec![sp()],
        ..Default::default()
    };
    let mut allowed = sample_doc();
    allowed.document_type = Some(dt());
    allowed.storage_path = Some(sp());
    let mut blocked = allowed.clone();
    blocked.correspondent = Some(c2());
    assert!(prefilter_matches(&allowed, &trig, None));
    assert!(!prefilter_matches(&blocked, &trig, None));
}

#[test]
fn test_prefilter_documents_applies_all_filters() {
    let other_dt = ObjRef::new(5, "Other Type");
    let other_sp = ObjRef::new(5, "Blocked path");
    let trig = WorkflowTrigger {
        filter_has_correspondent: Some(c()),
        filter_has_document_type: Some(dt()),
        filter_has_storage_path: Some(sp()),
        filter_has_tags: vec![t(1)],
        filter_has_all_tags: vec![t(1), t(2)],
        filter_has_not_tags: vec![t(3)],
        filter_has_not_correspondents: vec![c2()],
        filter_has_not_document_types: vec![other_dt.clone()],
        filter_has_not_storage_paths: vec![other_sp.clone()],
        ..Default::default()
    };
    let allowed = ExistingDocument {
        tags: vec![t(1), t(2)],
        correspondent: Some(c()),
        document_type: Some(dt()),
        storage_path: Some(sp()),
        original_filename: Some("allow.pdf".into()),
        ..Default::default()
    };
    let blocked = ExistingDocument {
        tags: vec![t(1), t(3)],
        correspondent: Some(c2()),
        document_type: Some(other_dt),
        storage_path: Some(other_sp),
        original_filename: Some("block.pdf".into()),
        ..Default::default()
    };
    assert!(prefilter_matches(&allowed, &trig, None));
    assert!(!prefilter_matches(&blocked, &trig, None));
}

#[test]
fn prefilter_null_semantics_and_filename_regex() {
    // exclude(correspondent__in=...) keeps documents without correspondent
    let trig = WorkflowTrigger { filter_has_not_correspondents: vec![c()], ..Default::default() };
    let mut d = sample_doc();
    d.correspondent = None;
    assert!(prefilter_matches(&d, &trig, None));
    // filename prefilter: anchored portable regex (upstream #14783), case-insensitive
    assert_eq!(filename_prefilter_regex("*.pdf"), r"^.*\.pdf$");
    let trig = WorkflowTrigger { filter_filename: Some("SAMPLE*".into()), ..Default::default() };
    let mut d = sample_doc();
    d.original_filename = Some("my_sample_1.pdf".into());
    assert!(!prefilter_matches(&d, &trig, None)); // anchored since #14783 (v3.4.2: suffix search)
    assert!(!existing_document_matches_workflow(&d, &trig, None).unwrap().0); // exact check agrees
    d.original_filename = Some("Sample_1.pdf".into());
    assert!(prefilter_matches(&d, &trig, None));
    d.original_filename = None;
    assert!(!prefilter_matches(&d, &trig, None));
}

#[test]
fn matching_any_with_workflow_trigger() {
    let trig = WorkflowTrigger { matching_algorithm: MATCH_ANY, match_text: "hello".into(), is_insensitive: true, ..Default::default() };
    assert!(existing_document_matches_workflow(&sample_doc(), &trig, None).unwrap().0);
}

#[test]
fn multiple_triggers_first_match_wins() {
    let no = WorkflowTrigger { id: 1, trigger_type: TRIGGER_DOCUMENT_UPDATED, filter_has_tags: vec![t(9)], ..Default::default() };
    let yes = WorkflowTrigger { id: 2, trigger_type: TRIGGER_DOCUMENT_UPDATED, ..Default::default() };
    let d = sample_doc();
    assert!(document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[no.clone(), yes], TRIGGER_DOCUMENT_UPDATED, None).unwrap());
    assert!(!document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[no], TRIGGER_DOCUMENT_UPDATED, None).unwrap());
    assert_eq!(
        document_matches_workflow(
            WorkflowDocument::Existing(&d),
            "W",
            &[WorkflowTrigger { trigger_type: 9, ..Default::default() }],
            9,
            None
        ),
        Err(WorkflowMatchError::UnsupportedTriggerType(9))
    );
}

/// PaperRust: trigger "Manual" (5, idea from paperless-ngx dev #14704) checks like the other
/// document triggers; a workflow without a type-5 trigger never matches on a manual run.
#[test]
fn manual_trigger_matches_like_existing_document_triggers() {
    let d = sample_doc();
    let manual = WorkflowTrigger { trigger_type: TRIGGER_MANUAL, filter_has_all_tags: vec![t(3)], ..Default::default() };
    assert!(document_matches_workflow(WorkflowDocument::Existing(&d), "W", std::slice::from_ref(&manual), TRIGGER_MANUAL, None).unwrap());
    let needs_other_tag = WorkflowTrigger { filter_has_all_tags: vec![t(4)], ..manual.clone() };
    assert!(!document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[needs_other_tag], TRIGGER_MANUAL, None).unwrap());
    let added_only = WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_ADDED, ..Default::default() };
    assert!(!document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[added_only], TRIGGER_MANUAL, None).unwrap());
    // a manual trigger does not apply on "document added"
    assert!(!document_matches_workflow(WorkflowDocument::Existing(&d), "W", &[manual], TRIGGER_DOCUMENT_ADDED, None).unwrap());
}
