//! Ported upstream tests (without DB).
//!
//! upstream: src/documents/tests/test_file_handling.py@v3.4.2, test_filters.py, test_workflows.py
//! (title templates), test_api_workflows.py (validation of assign_title), test_api_objects.py
//! (storage_paths/test). Each function names the upstream test; file system/DB parts (moving,
//! signals, trash) are the callers' business and are not re-implemented here.

use chrono::{FixedOffset, NaiveDate, TimeZone, Utc};

use crate::*;

fn opts(fmt: Option<&str>) -> Options {
    Options { filename_format: fmt.map(str::to_string), ..Options::default() }
}

fn opts_rn(fmt: Option<&str>) -> Options {
    Options { filename_format: fmt.map(str::to_string), filename_format_remove_none: true, ..Options::default() }
}

fn doc(pk: i64, title: &str) -> DocumentContext {
    let mut d = DocumentContext::new(pk, title, "application/pdf");
    d.created = NaiveDate::from_ymd_opt(2020, 6, 25).unwrap();
    d.added = Utc.with_ymd_and_hms(2024, 10, 1, 7, 36, 51).unwrap() + chrono::Duration::microseconds(153);
    d.modified = d.added;
    d
}

fn g(d: &DocumentContext, o: &Options) -> String {
    generate_filename(d, GenerateOptions::default(), o).unwrap()
}

fn gen_archive(d: &DocumentContext, o: &Options) -> String {
    generate_filename(d, GenerateOptions { archive_filename: true, ..Default::default() }, o).unwrap()
}

fn sp(path: &str) -> Option<StoragePathRef> {
    Some(StoragePathRef { id: Some(1), name: "sp".into(), path: path.into() })
}

fn corr(name: &str) -> Option<NamedRef> {
    Some(NamedRef { id: Some(1), name: name.into() })
}

fn cf(name: &str, ty: &str, value: CustomFieldValue) -> CustomFieldInstance {
    CustomFieldInstance { field_name: name.into(), data_type: ty.into(), select_options: None, value }
}

// ---------------------------------------------------------------------------
// TestFileHandling
// ---------------------------------------------------------------------------

#[test]
fn test_generate_unique_filename_renders_template_once() {
    let mut d = doc(1, "collision");
    d.filename = Some("collision_03.pdf".into());
    let existing = ["collision.pdf", "collision_01.pdf", "collision_02.pdf", "collision_03.pdf"];
    let o = opts(Some("{title}"));
    let r = generate_unique_filename(&d, false, &o, |p| existing.contains(&p)).unwrap();
    assert_eq!(r, "collision_03.pdf");
}

#[test]
fn test_generate_source_filename() {
    let d = doc(7, "x");
    assert_eq!(g(&d, &opts(Some(""))), "0000007.pdf");
}

#[test]
fn test_file_renaming() {
    let o = opts(Some("{correspondent}/{correspondent}"));
    let mut d = doc(1, "");
    assert_eq!(g(&d, &o), "none/none.pdf");
    d.correspondent = corr("test");
    assert_eq!(g(&d, &o), "test/test.pdf");
}

#[test]
fn test_document_type() {
    let o = opts(Some("{document_type} - {title}"));
    let mut d = doc(1, "the_doc");
    assert_eq!(g(&d, &o), "none - the_doc.pdf");
    d.document_type = corr("my_doc_type");
    assert_eq!(g(&d, &o), "my_doc_type - the_doc.pdf");
}

#[test]
fn test_asn() {
    let o = opts(Some("{asn} - {title}"));
    let mut d1 = doc(1, "the_doc");
    d1.archive_serial_number = Some(652);
    let d2 = doc(2, "the_doc");
    assert_eq!(g(&d1, &o), "652 - the_doc.pdf");
    assert_eq!(g(&d2, &o), "none - the_doc.pdf");
}

#[test]
fn test_tag_list() {
    let o = opts(Some("{title} {tag_list}"));
    let mut d = doc(1, "doc1");
    // order_by("name")
    d.tags = vec![TagRef { id: Some(2), name: "tag1".into() }, TagRef { id: Some(1), name: "tag2".into() }];
    assert_eq!(g(&d, &o), "doc1 tag1,tag2.pdf");
    let d2 = doc(2, "doc2");
    assert_eq!(g(&d2, &o), "doc2.pdf");
}

#[test]
fn test_filename_relative() {
    let o = opts(Some("//etc/something/{title}"));
    assert_eq!(g(&doc(1, "doc1"), &o), "etc/something/doc1.pdf");
}

#[test]
fn test_created_year_month_day() {
    let o = opts(Some("{created_year}-{created_month}-{created_day}"));
    let mut d = doc(1, "doc1");
    d.created = NaiveDate::from_ymd_opt(2020, 3, 6).unwrap();
    assert_eq!(g(&d, &o), "2020-03-06.pdf");
    d.created = NaiveDate::from_ymd_opt(2020, 11, 16).unwrap();
    assert_eq!(g(&d, &o), "2020-11-16.pdf");
}

#[test]
fn test_added_year_month_day() {
    let o = opts(Some("{added_year}-{added_month}-{added_day}"));
    let mut d = doc(1, "doc1");
    d.added = Utc.with_ymd_and_hms(1232, 1, 9, 1, 1, 1).unwrap();
    assert_eq!(g(&d, &o), "1232-01-09.pdf");
    d.added = Utc.with_ymd_and_hms(2020, 11, 16, 1, 1, 1).unwrap();
    assert_eq!(g(&d, &o), "2020-11-16.pdf");
}

#[test]
fn test_added_uses_local_date() {
    // timezone.localdate(document.added)
    let mut o = opts(Some("{added}"));
    o.time_zone = chrono_tz::Europe::Berlin;
    let mut d = doc(1, "x");
    d.added = Utc.with_ymd_and_hms(2020, 11, 16, 23, 30, 0).unwrap();
    assert_eq!(g(&d, &o), "2020-11-17.pdf");
}

#[test]
fn test_nested_directory_cleanup() {
    let o = opts(Some("{correspondent}/{correspondent}/{correspondent}"));
    assert_eq!(g(&doc(1, ""), &o), "none/none/none.pdf");
}

#[test]
fn test_format_doc_pk() {
    let o = opts(Some("{doc_pk}"));
    assert_eq!(g(&doc(1, ""), &o), "0000001.pdf");
    assert_eq!(g(&doc(13579, ""), &o), "0013579.pdf");
}

#[test]
fn test_format_none() {
    assert_eq!(g(&doc(1, ""), &opts(None)), "0000001.pdf");
}

#[test]
fn test_invalid_format() {
    let o = opts(Some("{% if x is None %}/{title]"));
    assert_eq!(g(&doc(1, ""), &o), "0000001.pdf");
}

#[test]
fn test_invalid_format_key() {
    let o = opts(Some("{created__year}"));
    assert_eq!(g(&doc(1, ""), &o), "0000001.pdf");
}

#[test]
fn test_duplicates() {
    let o = opts(Some("{title}"));
    let mut d1 = doc(1, "qwe");
    d1.filename = Some("0000001.pdf".into());
    let mut d2 = doc(2, "qwe");
    d2.filename = Some("0000002.pdf".into());
    let mut files: Vec<String> = vec!["0000001.pdf".into(), "0000002.pdf".into()];

    // document.save() -> rename
    let n1 = generate_unique_filename(&d1, false, &o, |p| files.iter().any(|f| f == p)).unwrap();
    assert_eq!(n1, "qwe.pdf");
    files.retain(|f| f != "0000001.pdf");
    files.push(n1.clone());
    d1.filename = Some(n1);

    let n2 = generate_unique_filename(&d2, false, &o, |p| files.iter().any(|f| f == p)).unwrap();
    assert_eq!(n2, "qwe_01.pdf");
    files.retain(|f| f != "0000002.pdf");
    files.push(n2.clone());
    d2.filename = Some(n2);

    // saving should not change the file names.
    assert_eq!(generate_unique_filename(&d1, false, &o, |p| files.iter().any(|f| f == p)).unwrap(), "qwe.pdf");
    assert_eq!(generate_unique_filename(&d2, false, &o, |p| files.iter().any(|f| f == p)).unwrap(), "qwe_01.pdf");

    // filename free, should remove _01 suffix
    files.retain(|f| f != "qwe.pdf");
    assert_eq!(generate_unique_filename(&d2, false, &o, |p| files.iter().any(|f| f == p)).unwrap(), "qwe.pdf");
}

#[test]
fn test_select_cf_updated() {
    let o = opts(Some("{{title}}_{{custom_fields|get_cf_value('test')}}"));
    let mut d = doc(1, "document");
    let options = |first: &str| {
        Some(vec![
            SelectOption { id: "abc123".into(), label: first.into() },
            SelectOption { id: "def456".into(), label: "banana".into() },
            SelectOption { id: "ghi789".into(), label: "cherry".into() },
        ])
    };
    d.custom_fields = vec![CustomFieldInstance {
        field_name: "test".into(),
        data_type: "select".into(),
        select_options: options("apple"),
        value: CustomFieldValue::Str("abc123".into()),
    }];
    assert_eq!(g(&d, &o), "document_apple.pdf");
    d.custom_fields[0].select_options = options("aubergine");
    assert_eq!(g(&d, &o), "document_aubergine.pdf");
}

#[test]
fn select_option_missing_is_context_error() {
    let o = opts(Some("{{title}}"));
    let mut d = doc(1, "document");
    d.custom_fields = vec![CustomFieldInstance {
        field_name: "test".into(),
        data_type: "select".into(),
        select_options: Some(vec![]),
        value: CustomFieldValue::Str("abc123".into()),
    }];
    assert!(matches!(generate_filename(&d, GenerateOptions::default(), &o), Err(TemplateError::Context(_))));
}

// ---------------------------------------------------------------------------
// TestFileHandlingWithArchive (name generation only)
// ---------------------------------------------------------------------------

#[test]
fn test_create_no_format() {
    let mut d = doc(1, "");
    d.filename = Some("0000001.pdf".into());
    d.archive_filename = Some("0000001.pdf".into());
    let o = opts(None);
    assert_eq!(generate_unique_filename(&d, false, &o, |_| true).unwrap(), "0000001.pdf");
    assert_eq!(generate_unique_filename(&d, true, &o, |_| true).unwrap(), "0000001.pdf");
}

#[test]
fn test_create_with_format() {
    let o = opts(Some("{correspondent}/{title}"));
    let mut d = doc(1, "my_doc");
    d.filename = Some("0000001.pdf".into());
    d.archive_filename = Some("0000001.pdf".into());
    let new = generate_unique_filename(&d, false, &o, |p| p == "0000001.pdf").unwrap();
    assert_eq!(new, "none/my_doc.pdf");
    // The handler sets filename before the archive name.
    d.filename = Some(new);
    let arch = generate_unique_filename(&d, true, &o, |p| p == "0000001.pdf").unwrap();
    assert_eq!(arch, "none/my_doc.pdf");
}

#[test]
fn archive_unique_with_counter() {
    let o = opts(Some("{title}"));
    let mut d = doc(1, "a");
    d.filename = Some("a.png".into());
    d.mime_type = "image/png".into();
    d.file_type = ".png".into();
    // simple name a.pdf taken -> counter on the archive name
    let r = generate_unique_filename(&d, true, &o, |p| p == "a.pdf").unwrap();
    assert_eq!(r, "a_01.pdf");
}

// ---------------------------------------------------------------------------
// TestFilenameGeneration
// ---------------------------------------------------------------------------

#[test]
fn test_invalid_characters() {
    let o = opts(Some("{title}"));
    assert_eq!(g(&doc(1, "This. is the title."), &o), "This. is the title.pdf");
    assert_eq!(g(&doc(2, "my\\invalid/../title:yay"), &o), "my-invalid-..-title-yay.pdf");
}

#[test]
fn test_date() {
    let o = opts(Some("{created}"));
    let mut d = doc(2, "does not matter");
    d.created = NaiveDate::from_ymd_opt(2020, 5, 21).unwrap();
    assert_eq!(g(&d, &o), "2020-05-21.pdf");
}

#[test]
fn test_dynamic_path() {
    let mut d = doc(2, "does not matter");
    d.storage_path = sp("TestFolder/{{created}}");
    assert_eq!(g(&d, &opts(None)), "TestFolder/2020-06-25.pdf");
}

#[test]
fn test_dynamic_path_with_none() {
    let mut d = doc(2, "does not matter");
    d.storage_path = sp("{{asn}} - {{created}}");
    assert_eq!(g(&d, &opts(None)), "none - 2020-06-25.pdf");
}

#[test]
fn test_dynamic_path_remove_none() {
    let o = opts_rn(None);
    let mut d = doc(2, "does not matter");
    d.storage_path = sp("TestFolder/{{asn}}/{{created}}");
    assert_eq!(g(&d, &o), "TestFolder/2020-06-25.pdf");
    d.storage_path = sp("{{ owner_username }}/{{ created_year }}/{{ correspondent }}/{{ title }}");
    assert_eq!(g(&d, &o), "2020/does not matter.pdf");
}

#[test]
fn test_multiple_doc_paths() {
    let mut a = doc(2, "does not matter");
    a.archive_serial_number = Some(4);
    a.storage_path = sp("ThisIsAFolder/{{asn}}/{{created}}");
    let mut b = doc(5, "does not matter");
    b.created = NaiveDate::from_ymd_opt(2020, 7, 25).unwrap();
    b.storage_path = sp("SomeImportantNone/{{created}}");
    assert_eq!(g(&a, &opts(None)), "ThisIsAFolder/4/2020-06-25.pdf");
    assert_eq!(g(&b, &opts(None)), "SomeImportantNone/2020-07-25.pdf");
}

#[test]
fn test_no_path_fallback() {
    let mut a = doc(2, "does not matter");
    a.archive_serial_number = Some(4);
    let mut b = doc(5, "does not matter");
    b.created = NaiveDate::from_ymd_opt(2020, 7, 25).unwrap();
    b.storage_path = sp("SomeImportantNone/{{created}}");
    assert_eq!(g(&a, &opts(None)), "0000002.pdf");
    assert_eq!(g(&b, &opts(None)), "SomeImportantNone/2020-07-25.pdf");
}

#[test]
fn test_placeholder_matches_none_variants_and_false() {
    let o = opts(Some(concat!(
        "{% if correspondent == 'none' %}none/{% endif %}",
        "{% if correspondent == '-none-' %}dash/{% endif %}",
        "{% if not correspondent %}false/{% endif %}",
        "{% if correspondent != 'none' %}notnoneyes/{% else %}notnoneno/{% endif %}",
        "{{ correspondent or 'missing' }}/{{ title }}"
    )));
    let without = doc(1, "does not matter");
    let mut with = doc(2, "does not matter");
    with.correspondent = corr("Acme");
    assert_eq!(g(&without, &o), "none/dash/false/notnoneno/missing/does not matter.pdf");
    assert_eq!(g(&with, &o), "notnoneyes/Acme/does not matter.pdf");
}

#[test]
fn test_short_names_created() {
    let o = opts(Some("{created_year_short}/{created_month_name_short}/{created_month_name}/{title}"));
    let mut d = doc(2, "The Title");
    d.created = NaiveDate::from_ymd_opt(1989, 12, 2).unwrap();
    assert_eq!(g(&d, &o), "89/Dec/December/The Title.pdf");
}

#[test]
fn test_short_names_added() {
    let o = opts(Some("{added_year_short}/{added_month_name}/{added_month_name_short}/{title}"));
    let mut d = doc(2, "The Title");
    d.added = Utc.with_ymd_and_hms(1984, 8, 21, 7, 36, 51).unwrap();
    assert_eq!(g(&d, &o), "84/August/Aug/The Title.pdf");
}

#[test]
fn test_document_owner_string() {
    let o = opts(Some("{owner_username}/{title}"));
    let mut owned = doc(1, "The Title");
    owned.owner = Some(OwnerRef { id: 1, username: "user1".into() });
    let no_owner = doc(2, "does matter");
    assert_eq!(g(&owned, &o), "user1/The Title.pdf");
    assert_eq!(g(&no_owner, &o), "none/does matter.pdf");
}

#[test]
fn test_document_original_filename() {
    let o = opts(Some("{original_name}"));
    let mut a = doc(1, "does matter");
    a.original_filename = Some("someepdf.pdf".into());
    let mut b = doc(2, "does matter");
    b.original_filename = Some("some pdf with spaces and stuff.pdf".into());
    let c = doc(3, "does matter");
    let mut t = DocumentContext::new(4, "does matter", "text/plain");
    t.original_filename = Some("logs.txt".into());
    assert_eq!(g(&a, &o), "someepdf.pdf");
    assert_eq!(g(&b, &o), "some pdf with spaces and stuff.pdf");
    assert_eq!(g(&c, &o), "none.pdf");
    assert_eq!(g(&t, &o), "logs.txt");
    assert_eq!(gen_archive(&t, &o), "logs.pdf");
}

fn version_of(root: DocumentContext, pk: i64, title: &str, idx: i64) -> DocumentContext {
    let mut v = doc(pk, title);
    v.file_type = root.file_type.clone();
    v.mime_type = root.mime_type.clone();
    v.root_document = Some(Box::new(root));
    v.version_index = Some(idx);
    v
}

#[test]
fn test_version_index_suffix_for_template_filename() {
    let o = opts(Some("{title}"));
    let v = version_of(doc(1, "the_doc"), 2, "the_doc", 1);
    assert_eq!(g(&v, &o), "the_doc_v1.pdf");
    assert_eq!(generate_filename(&v, GenerateOptions { counter: 1, ..Default::default() }, &o).unwrap(), "the_doc_v1_01.pdf");
}

#[test]
fn test_version_index_suffix_for_default_filename() {
    let root = DocumentContext::new(3, "root", "text/plain");
    let v = version_of(root, 4, "root", 2);
    assert_eq!(g(&v, &opts(None)), "0000003_v2.txt");
    assert_eq!(gen_archive(&v, &opts(None)), "0000003_v2.pdf");
}

#[test]
fn test_version_index_suffix_with_original_name_placeholder() {
    let o = opts(Some("{original_name}"));
    let mut root = doc(1, "root");
    root.original_filename = Some("root-upload.pdf".into());
    let mut v = version_of(root, 2, "root", 1);
    v.original_filename = Some("version-upload.pdf".into());
    assert_eq!(g(&v, &o), "root-upload_v1.pdf");
}

#[test]
fn test_version_index_suffix_with_storage_path() {
    let mut root = doc(1, "storage_doc");
    root.storage_path = sp("folder/{{title}}");
    let v = version_of(root, 2, "version_title_should_not_be_used", 3);
    assert_eq!(g(&v, &opts(None)), "folder/storage_doc_v3.pdf");
}

#[test]
fn test_remove_none_not_dir() {
    let o = opts_rn(Some("XX{correspondent}/{title}"));
    assert_eq!(g(&doc(1, "doc1"), &o), "XX/doc1.pdf");
}

/// Security review 2026-11-19 F12: storage path with stream/device names and a dot at the folder end
/// yields a sanitized name on Windows (no error), on Linux like upstream.
#[test]
fn test_windows_reserved_names_in_storage_path() {
    let mut d = doc(1, "doc1");
    d.storage_path = sp("a:b/CON/x./{{ title }}:strom");
    let want = if cfg!(windows) { "a-b/CON_/x/doc1-strom.pdf" } else { "a:b/CON/x./doc1:strom.pdf" };
    assert_eq!(g(&d, &opts(None)), want);
    d.storage_path = sp("nul");
    assert_eq!(g(&d, &opts(None)), if cfg!(windows) { "nul_.pdf" } else { "nul.pdf" });
}

#[test]
fn test_remove_none_cannot_create_traversal() {
    let mut d = doc(9, "doc1");
    d.storage_path = sp(".-none-./.-none-./tmp/pwned");
    let r = g(&d, &opts_rn(None));
    assert!(!r.split('/').any(|p| p == ".."));
    assert_eq!(r, "0000009.pdf");
}

#[test]
fn test_remove_none_still_removes_placeholder() {
    let mut d = doc(1, "doc1");
    d.storage_path = sp("{{ correspondent }}/{{ title }}");
    assert_eq!(g(&d, &opts_rn(None)), "doc1.pdf");
}

#[test]
fn test_remove_none_cannot_create_traversal_from_metadata() {
    let o = opts_rn(Some("{{ correspondent }}/{{ title }}/{{ doc_pk }}"));
    let d = doc(9, ".-none-.");
    let r = g(&d, &o);
    assert!(!r.split('/').any(|p| p == ".."));
    assert_eq!(r, "0000009.pdf");
}

#[test]
fn test_complex_template_strings() {
    let mut d = doc(2, "Does Matter");
    d.checksum = "2".into();
    d.archive_serial_number = Some(25);
    d.storage_path = sp("
                 somepath/
                 {% if document.checksum == '2' %}
                   some where/{{created}}
                 {% else %}
                   {{added}}
                 {% endif %}
                 /{{ title }}
                 ");
    let o = opts(None);
    assert_eq!(g(&d, &o), "somepath/some where/2020-06-25/Does Matter.pdf");
    d.checksum = "5".into();
    assert_eq!(g(&d, &o), "somepath/2024-10-01/Does Matter.pdf");

    d.storage_path = sp("{{ document.title|lower }}{{ document.archive_serial_number - 2 }}");
    assert_eq!(g(&d, &o), "does matter23.pdf");

    d.storage_path = sp("
                 somepath/
                 {% if document.archive_serial_number >= 0 and document.archive_serial_number <= 200 %}
                   asn-000-200/{{title}}
                 {% elif document.archive_serial_number >= 201 and document.archive_serial_number <= 400 %}
                   asn-201-400
                   {% if document.archive_serial_number >= 201 and document.archive_serial_number < 300 %}
                     /asn-2xx
                   {% elif document.archive_serial_number >= 300 and document.archive_serial_number < 400 %}
                     /asn-3xx
                   {% endif %}
                 {% endif %}
                 /{{ title }}
                 ");
    assert_eq!(g(&d, &o), "somepath/asn-000-200/Does Matter/Does Matter.pdf");
    d.archive_serial_number = Some(301);
    assert_eq!(g(&d, &o), "somepath/asn-201-400/asn-3xx/Does Matter.pdf");
}

#[test]
fn test_template_related_context_keeps_legacy_string_coercion() {
    let mut d = doc(1, "scan_017562");
    d.created = NaiveDate::from_ymd_opt(2025, 7, 2).unwrap();
    d.storage_path = Some(StoragePathRef {
        id: Some(1),
        name: "PARTNER".into(),
        path: concat!(
            "{{ document.storage_path|lower }} / ",
            "{{ document.correspondent|lower|replace('mi:', 'mieter/') }} / ",
            "{{ document_type|lower }} / ",
            "{{ title|lower }}"
        )
        .into(),
    });
    d.correspondent = corr("mi:kochkach");
    d.document_type = corr("Mietvertrag");
    assert_eq!(g(&d, &opts(None)), "partner/mieter/kochkach/mietvertrag/scan_017562.pdf");
}

#[test]
fn test_template_with_undefined_var() {
    let mut d = doc(2, "Does Matter");
    d.archive_serial_number = Some(25);
    let o = opts(Some("{{creation_date}}/{{ title_name_str }}"));
    assert_eq!(g(&d, &o), "0000002.pdf");
    assert!(matches!(
        validate_filepath_template_and_render("{{creation_date}}/{{ title_name_str }}", Some(&d), &o),
        Err(TemplateError::Undefined(_))
    ));
}

#[test]
fn test_template_with_security() {
    let d = doc(2, "Does Matter");
    let o = opts(Some("{{created}}/{{ document.save() }}"));
    assert_eq!(g(&d, &o), "0000002.pdf");
}

#[test]
fn test_template_with_custom_fields() {
    let mut d = doc(2, "Some Title");
    d.archive_serial_number = Some(25);
    let select = CustomFieldInstance {
        field_name: "Select Field".into(),
        data_type: "select".into(),
        select_options: Some(vec![
            SelectOption { id: "abc=123".into(), label: "ChoiceOne".into() },
            SelectOption { id: "def-456".into(), label: "ChoiceTwo".into() },
        ]),
        value: CustomFieldValue::Str("abc=123".into()),
    };
    d.custom_fields = vec![select, cf("Invoice", "integer", CustomFieldValue::Int(1234))];

    let o = opts(Some(
        "
                 {% if \"Invoice\" in custom_fields %}
                   invoices/{{ custom_fields | get_cf_value('Invoice') }}
                 {% else %}
                   not-invoices/{{ title }}
                 {% endif %}
                 ",
    ));
    assert_eq!(g(&d, &o), "invoices/1234.pdf");

    let o = opts(Some(
        "
                 {% if \"Select Field\" in custom_fields %}
                   {{ title }}_{{ custom_fields | get_cf_value('Select Field', 'Default Value') }}
                 {% else %}
                   {{ title }}
                 {% endif %}
                 ",
    ));
    assert_eq!(g(&d, &o), "Some Title_ChoiceOne.pdf");
    d.custom_fields[0].value = CustomFieldValue::Null;
    assert_eq!(g(&d, &o), "Some Title_Default Value.pdf");

    d.custom_fields[1] = cf("Invoice Number", "integer", CustomFieldValue::Int(4567));
    let o = opts(Some("invoices/{{ custom_fields | get_cf_value('Invoice Number') }}"));
    assert_eq!(g(&d, &o), "invoices/4567.pdf");
    let o = opts(Some("invoices/{{ custom_fields | get_cf_value('Ince Number', 0) }}"));
    assert_eq!(g(&d, &o), "invoices/0.pdf");
}

#[test]
fn test_datetime_filter() {
    let mut d = doc(2, "Some Title");
    d.custom_fields = vec![cf("Invoice Date", "date", CustomFieldValue::Date(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()))];
    assert_eq!(g(&d, &opts(Some("{{ created | datetime('%Y') }}/{{ title }}"))), "2020/Some Title.pdf");
    assert_eq!(g(&d, &opts(Some("{{ created | datetime('%Y-%m-%d') }}/{{ title }}"))), "2020-06-25/Some Title.pdf");
    assert_eq!(
        g(&d, &opts(Some("{{ custom_fields | get_cf_value('Invoice Date') | datetime('%Y-%m-%d') }}/{{ title }}"))),
        "2024-10-01/Some Title.pdf"
    );
}

#[test]
fn test_slugify_filter() {
    let mut d = doc(2, "Some Title! With @ Special # Characters");
    assert_eq!(g(&d, &opts(Some("{{ title | slugify }}"))), "some-title-with-special-characters.pdf");
    d.correspondent = corr("John's @ Office / Workplace");
    assert_eq!(
        g(&d, &opts(Some("{{ correspondent | slugify }}/{{ title | slugify }}"))),
        "johns-office-workplace/some-title-with-special-characters.pdf"
    );
    d.custom_fields = vec![cf("Location", "string", CustomFieldValue::Str("Brussels @ Belgium!".into()))];
    assert_eq!(
        g(&d, &opts(Some("{{ custom_fields | get_cf_value('Location') | slugify }}/{{ title | slugify }}"))),
        "brussels-belgium/some-title-with-special-characters.pdf"
    );
}

// ---------------------------------------------------------------------------
// TestCustomFieldFilenameUpdates / TestPathDateLocalization
// ---------------------------------------------------------------------------

#[test]
fn test_custom_field_in_template_triggers_filename_update() {
    let mut d = doc(1, "document");
    d.storage_path = sp("{{ custom_fields|get_cf_value('flavor') }}/{{ title }}");
    d.custom_fields = vec![cf("flavor", "string", CustomFieldValue::Str("initial".into()))];
    assert_eq!(g(&d, &opts(None)), "initial/document.pdf");
    d.custom_fields[0].value = CustomFieldValue::Str("updated".into());
    assert_eq!(g(&d, &opts(None)), "updated/document.pdf");
}

#[test]
fn test_overlong_storage_path_keeps_existing_filename() {
    // The handler (pl-documents-write) discards names over MAX_STORED_FILENAME_LENGTH.
    let mut d = doc(1, "document");
    d.storage_path = sp(&"a".repeat(1100));
    assert!(g(&d, &opts(None)).len() > MAX_STORED_FILENAME_LENGTH);
}

#[test]
fn test_localize_date_path_building() {
    let mut d = doc(1, "My Document");
    d.created = NaiveDate::from_ymd_opt(2023, 10, 26).unwrap();
    for (fmt, expected) in [
        ("{{title}}_{{ document.created | localize_date('MMMM', 'es_ES')}}", "My Document_octubre.pdf"),
        ("{{title}}_{{ document.created | localize_date('EEEE', 'fr_FR')}}", "My Document_jeudi.pdf"),
        ("{{title}}_{{ document.created | localize_date('dd/MM/yyyy', 'en_GB')}}", "My Document_26/10/2023.pdf"),
    ] {
        assert_eq!(g(&d, &opts(Some(fmt))), expected, "{fmt}");
    }
}

// ---------------------------------------------------------------------------
// test_filters.py (via the template)
// ---------------------------------------------------------------------------

fn wf(created: Option<NaiveDate>) -> WorkflowPlaceholders {
    WorkflowPlaceholders {
        correspondent_name: Some(String::new()),
        doc_type_name: Some(String::new()),
        owner_username: Some(String::new()),
        local_added: FixedOffset::east_opt(0).unwrap().with_ymd_and_hms(2023, 6, 26, 14, 30, 5).unwrap(),
        original_filename: "sample.pdf".into(),
        filename: "".into(),
        created,
        doc_title: Some(String::new()),
        doc_url: Some(String::new()),
        doc_id: Some(1),
        custom_fields: vec![],
    }
}

fn wrender(t: &str) -> Result<Option<String>, TemplateError> {
    parse_w_workflow_placeholders(t, &wf(NaiveDate::from_ymd_opt(2023, 6, 26)))
}

#[test]
fn test_localize_date_with_strings() {
    for v in ["2023-10-26T14:30:05+00:00", "2023-10-26"] {
        for (fmt, loc, exp) in [
            ("EEEE, MMM d, yyyy", "en_US", "Thursday, Oct 26, 2023"),
            ("dd.MM.yyyy", "de_DE", "26.10.2023"),
            ("EEEE", "de_DE", "Donnerstag"),
            ("MMMM", "de_DE", "Oktober"),
            ("EEEE", "fr_FR", "jeudi"),
            ("MMMM", "fr_FR", "octobre"),
        ] {
            let t = format!("{{{{ '{v}' | localize_date('{fmt}', '{loc}') }}}}");
            assert_eq!(wrender(&t).unwrap().unwrap(), exp, "{t}");
        }
    }
}

#[test]
fn test_localize_date_raises_type_error_for_invalid_input() {
    for v in ["1698330605", "none", "[]", "{}"] {
        let t = format!("{{{{ {v} | localize_date('medium', 'en_US') }}}}");
        match wrender(&t) {
            Err(TemplateError::Other(m)) => assert!(m.contains("Unsupported type <class '"), "{m}"),
            other => panic!("{t}: {other:?}"),
        }
    }
}

#[test]
fn test_localize_date_raises_error_for_invalid_locale() {
    match wrender("{{ created | localize_date('medium', 'invalid_locale_code') }}") {
        Err(TemplateError::Other(m)) => assert!(m.contains("Invalid locale identifier"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_format_datetime_parses_iso_strings() {
    assert_eq!(wrender("{{ '2026-09-21' | datetime('%Y-%m-%d %H:%M') }}").unwrap().unwrap(), "2026-09-21 00:00");
    assert_eq!(wrender("{{ '2026-09-21T13:45:00+00:00' | datetime('%Y-%m-%d %H:%M') }}").unwrap().unwrap(), "2026-09-21 13:45");
    match wrender("{{ 'not a date' | datetime('%Y') }}") {
        Err(TemplateError::Other(m)) => assert!(m.contains("Invalid date or datetime string: 'not a date'"), "{m}"),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Workflows (test_workflows.py / test_api_workflows.py)
// ---------------------------------------------------------------------------

#[test]
fn test_title_custom_fields() {
    let t = "{{ custom_fields.Invoice.value }} {{ custom_fields | get_cf_value(\"Missing\", \"fallback\") }}";
    assert!(validate_workflow_template(t).is_ok());
    let mut p = wf(None);
    p.custom_fields = vec![cf("Invoice", "string", CustomFieldValue::Str("INV-123".into()))];
    assert_eq!(parse_w_workflow_placeholders(t, &p).unwrap().unwrap(), "INV-123 fallback");
}

#[test]
fn test_workflow_placeholders_basic() {
    let mut p = wf(NaiveDate::from_ymd_opt(2014, 1, 5));
    p.correspondent_name = Some("Corr".into());
    assert_eq!(render_workflow_title("Doc from {{correspondent}}", &p).unwrap(), "Doc from Corr");
    assert_eq!(render_workflow_title("Doc created in {{created_year}}", &p).unwrap(), "Doc created in 2014");
    assert_eq!(render_workflow_title("Doc added in {{added_month_name_short}}", &p).unwrap(), "Doc added in Jun");
    assert_eq!(
        render_workflow_title("{{ added }} {{ added_time }} {{ created_time }}", &p).unwrap(),
        "2023-06-26T14:30:05+00:00 14:30 00:00"
    );
    // old syntax is not converted for workflows
    assert_eq!(render_workflow_title("Doc from {correspondent}", &p).unwrap(), "Doc from {correspondent}");
    // Consumer without correspondent: None
    p.correspondent_name = None;
    assert_eq!(render_workflow_title("Doc from {{correspondent}}", &p).unwrap(), "Doc from None");
    assert_eq!(render_workflow_title("{{ original_filename }}/{{ doc_id }}", &p).unwrap(), "sample/1");
}

#[test]
fn workflow_doc_id_variants() {
    // build_workflow_action_context: on ingestion "id": "" → empty text; None → undefined
    let p = wf(None);
    let r = |id: WorkflowDocId| parse_w_workflow_placeholders_with_id("[{{ doc_id }}]", &p, &id);
    assert_eq!(r(WorkflowDocId::Empty).unwrap().unwrap(), "[]");
    assert_eq!(r(WorkflowDocId::Id(7)).unwrap().unwrap(), "[7]");
    assert!(matches!(r(WorkflowDocId::Unset), Err(TemplateError::Undefined(_))));
    assert_eq!(
        parse_w_workflow_placeholders_with_id("{% if doc_id is defined %}d{% else %}u{% endif %}", &p, &WorkflowDocId::Empty)
            .unwrap()
            .unwrap(),
        "d"
    );
    assert_eq!(WorkflowDocId::from(Some(3)), WorkflowDocId::Id(3));
    assert_eq!(WorkflowDocId::from(None), WorkflowDocId::Unset);
}

#[test]
fn test_document_added_invalid_title_placeholders() {
    assert_eq!(render_workflow_title("Doc {created_year]", &wf(None)).unwrap(), "Doc {created_year]");
}

#[test]
fn test_document_added_malformed_title_template_falls_back() {
    assert_eq!(parse_w_workflow_placeholders("Doc {{ unclosed", &wf(None)).unwrap(), None);
    assert_eq!(render_workflow_title("Doc {{ unclosed", &wf(None)), None);
}

#[test]
fn test_workflow_undefined_raises() {
    assert!(matches!(parse_w_workflow_placeholders("{{ nope }}", &wf(None)), Err(TemplateError::Undefined(_))));
    // created is not set without a date
    assert!(matches!(parse_w_workflow_placeholders("{{ created }}", &wf(None)), Err(TemplateError::Undefined(_))));
}

#[test]
fn test_workflow_localization() {
    for (t, exp) in [
        ("Created at {{ created | localize_date('MMMM', 'es_ES') }}", "Created at junio"),
        ("Created at {{ created | localize_date('MMMM', 'de_DE') }}", "Created at Juni"),
        ("Created at {{ created | localize_date('dd/MM/yyyy', 'en_GB') }}", "Created at 26/06/2023"),
        ("Added at {{ added | localize_date('MMMM', 'es_ES') }}", "Added at junio"),
        ("Added at {{ added | localize_date('MMMM', 'de_DE') }}", "Added at Juni"),
        ("Added at {{ added | localize_date('dd/MM/yyyy', 'en_GB') }}", "Added at 26/06/2023"),
    ] {
        assert_eq!(wrender(t).unwrap().unwrap(), exp, "{t}");
    }
}

#[test]
fn test_api_create_complex_assign_title() {
    let t = "{# this is a comment #}foo{% if created_year < 2000 %}bar{% endif %}{{ \"{:04d}\".format(42) }}";
    assert!(validate_workflow_template(t).is_ok());
}

#[test]
fn test_api_create_invalid_assign_title_syntax_error() {
    let e = validate_workflow_template("{{created_year}").unwrap_err();
    assert!(e.message().starts_with("Template syntax error"), "{e}");
}

#[test]
fn test_api_create_invalid_assign_title_assertion_error() {
    let e = validate_workflow_template("{{ created_year | foo }}").unwrap_err();
    assert_eq!(e.message(), "Template assertion error: No filter named 'foo'.\n  File \"<introspection>\", line 1");
    let e = validate_workflow_template("{{ x is None }}").unwrap_err();
    assert_eq!(e.message(), "Template assertion error: No test named 'None'.\n  File \"<introspection>\", line 1");
    let e = validate_workflow_template("{{ x|split }}").unwrap_err();
    assert!(e.message().contains("No filter named 'split'"));
}

#[test]
fn test_api_create_invalid_assign_title_unknown_placeholder() {
    let e = validate_workflow_template("{{creation_year}}").unwrap_err();
    assert!(matches!(e, WorkflowTemplateError::KeyError(_)));
    assert!(e.message().contains("Template references unknown placeholders"));
    assert!(e.message().contains("creation_year"));
}

#[test]
fn validate_workflow_template_globals_and_scopes() {
    assert!(validate_workflow_template("{{ range(3)|join }}{% for z in custom_fields %}{{ z }}{{ loop.index }}{% endfor %}").is_ok());
    assert!(validate_workflow_template("{% set y = 1 %}{{ y }}{{ doc_title }}").is_ok());
    let e = validate_workflow_template("{{ b }}{{ a }}").unwrap_err();
    assert_eq!(e.message(), "Template references unknown placeholders: a, b");
}

// ---------------------------------------------------------------------------
// storage_paths (test_api_objects.py)
// ---------------------------------------------------------------------------

#[test]
fn test_api_storage_path_placeholders() {
    let path = "{title}/{correspondent}/{document_type}/{created}/{created_year}/{created_year_short}/{created_month}/{created_month_name}/{created_month_name_short}/{created_day}/{added}/{added_year}/{added_year_short}/{added_month}/{added_month_name}/{added_month_name_short}/{added_day}/{asn}/{tag_list}/{owner_username}/{original_name}/{doc_pk}/";
    let converted = convert_format_str_to_template_format(path);
    assert!(validate_filepath_template_and_render(&converted, None, &Options::default()).is_ok());
    // invalid variable -> "Invalid variable detected."
    assert!(validate_filepath_template_and_render("{{ nope }}", None, &Options::default()).is_err());
}

#[test]
fn test_test_storage_path() {
    let mut d = doc(1, "Something");
    d.checksum = "123".into();
    assert_eq!(storage_path_test_result(&d, "path/{{ title }}", &opts(None)).unwrap().as_deref(), Some("path/Something.pdf"));
}

#[test]
fn test_test_storage_path_respects_none_placeholder_setting() {
    let d = doc(1, "Something");
    let path = "folder/{{ correspondent }}/{{ title }}";
    assert_eq!(storage_path_test_result(&d, path, &opts(None)).unwrap().as_deref(), Some("folder/none/Something.pdf"));
    assert_eq!(storage_path_test_result(&d, path, &opts_rn(None)).unwrap().as_deref(), Some("folder/Something.pdf"));
    assert_eq!(storage_path_test_result(&d, "{{ nope }}", &opts(None)).unwrap(), None);
    let mut e = d.clone();
    e.filename = Some("x/old.PNG".into());
    assert_eq!(storage_path_test_result(&e, "a/{{ title }}", &opts(None)).unwrap().as_deref(), Some("a/Something.PNG"));
}

/// B-N1: `storage_path_test_response` returns all three upstream cases from a single rendering
/// (empty → `""`, error → `None`, otherwise with extension).
#[test]
fn storage_path_test_response_single_render_semantics() {
    let d = doc(1, "Something");
    assert_eq!(storage_path_test_response(&d, "{% if false %}x{% endif %}", &opts(None)).unwrap().as_deref(), Some(""));
    assert_eq!(storage_path_test_result(&d, "{% if false %}x{% endif %}", &opts(None)).unwrap(), None);
    assert_eq!(storage_path_test_response(&d, "{{ nope }}", &opts(None)).unwrap(), None);
    assert_eq!(storage_path_test_response(&d, "p/{{ title }}", &opts(None)).unwrap().as_deref(), Some("p/Something.pdf"));
}

// ---------------------------------------------------------------------------
// Sandbox / security
// ---------------------------------------------------------------------------

#[test]
fn sandbox_limits() {
    let d = doc(1, "t");
    let o = Options::default();
    let big = validate_filepath_template_and_render("{{ 'a' * 100000000000 }}", Some(&d), &o);
    assert!(matches!(big, Err(TemplateError::Security(_))), "{big:?}");
    let grow = validate_filepath_template_and_render(
        "{% set ns = namespace(s='ab') %}{% for i in range(100) %}{% set ns.s = ns.s ~ ns.s %}{% endfor %}{{ ns.s }}",
        Some(&d),
        &o,
    );
    assert!(matches!(grow, Err(TemplateError::Security(_))), "{grow:?}");
    let spin = validate_filepath_template_and_render(
        "{% for i in range(100000) %}{% for j in range(100000) %}{% endfor %}{% endfor %}",
        Some(&d),
        &o,
    );
    assert!(matches!(spin, Err(TemplateError::Security(_))), "{spin:?}");
    let range = validate_filepath_template_and_render("{{ range(200000)|length }}", Some(&d), &o);
    assert!(range.is_err());
    let out = validate_filepath_template_and_render(
        "{% for i in range(100000) %}{{ document.content }}{% endfor %}",
        Some(&DocumentContext { content: "x".repeat(1000), ..d.clone() }),
        &o,
    );
    assert!(matches!(out, Err(TemplateError::Security(_))), "{out:?}");
    for t in ["{{ document.save() }}", "{{ custom_fields.update({}) }}", "{{ document.__class__ }}", "{{ title.__class__ }}"] {
        assert!(validate_filepath_template_and_render(t, Some(&d), &o).is_err(), "{t}");
    }
    assert_eq!(validate_filepath_template_and_render("/abs/{{ title }}", Some(&d), &o).unwrap(), "abs/t");
    assert_eq!(validate_filepath_template_and_render("a/../{{ title }}", Some(&d), &o), Err(TemplateError::UnsafePath));
}

// ---------------------------------------------------------------------------
// Jinja2 behavior (expectations generated with Jinja2 3.1.6, see contract)
// ---------------------------------------------------------------------------

#[test]
fn jinja2_compat() {
    let mut d = doc(5, "Hello World");
    d.tags = vec![TagRef { id: Some(1), name: "b".into() }, TagRef { id: Some(2), name: "a".into() }];
    let o = Options::default();
    let mut fails: Vec<String> = Vec::new();
    let mut r = |t: &str, exp: &str| match validate_filepath_template_and_render(t, Some(&d), &o) {
        Ok(v) if v == exp => {}
        other => fails.push(format!(
            "{t}
   got {other:?}
  want {exp:?}"
        )),
    };
    r("{{ document.title|truncate(9) }}", "Hello World");
    r("{{ 'abcdefghijklmnop'|truncate(9) }}", "abcdef...");
    r("[{{ 'a'|center(4) }}]", "[ a  ]");
    r("{{ 2.5|round }}|{{ 3.5|round }}|{{ 7|round }}", "2.0|4.0|7");
    r("{{ 'x y-z(a'|title }}", "X Y-Z(A");
    r("{{ '42'|int }}|{{ '4.7'|int }}|{{ 'abc'|int }}|{{ '1e3'|float }}", "42|4|0|1000.0");
    r("{{ '%05.1f|%-4s|%d' | format(3.14159, 'ab', 7) }}", "003.1|ab  |7");
    r(
        "{{ document.title.split()[1] }}|{{ document.title.replace('o','0') }}|{{ document.title.startswith(('x','H')) }}|{{ '{:>8}'.format(document.title[:3]) }}",
        "World|Hell0 W0rld|True|     Hel",
    );
    r("{{ tag_name_list|join(',') }}|{{ tag_name_list|sort|first }}|{{ tag_name_list|length }}", "b,a|a|2");
    r("{% for k, v in {'a': 1}.items() %}{{ k }}={{ v }}{% endfor %}", "a=1");
    r("{{ 10 // 3 }}|{{ -7 // 2 }}|{{ 2 ** -1 }}|{{ 1 / 3 }}", "3|-4|0.5|0.3333333333333333");
    r("{{ document.title[1:3] }}|{{ document.title[-1] }}|{{ tag_name_list[::-1] }}", "el|d|['a', 'b']");
    r("{{ 'x' ~ 1 ~ none }}", "x1None");
    r("{{ 5 is divisibleby 5 }}|{{ 5 is odd }}|{{ none is none }}|{{ title is string }}", "True|True|True|True");
    r("{% if title is defined and title|length > 3 %}yes{% endif %}", "yes");
    r("{{ [1,2]|sum }}|{{ [1,2]|max }}", "3|2");
    r("{{ 'a,b'|replace(',', '-') }}|{{ document.title|wordcount }}|{{ title|upper }}", "a-b|2|HELLO WORLD");
    r("{{ true }}{{ True }}{{ none }}{{ None }}", "TrueTrueNoneNone");
    r("{{ document.page_count }}|{{ document.owner }}|{{ document.tags[0].name }}", "None|None|b");
    r("{{ document.added }}|{{ document.created }}|{{ document.created.year }}", "2024-10-01 07:36:51.000153+00:00|2020-06-25|2020");
    r("{{ document.created.strftime('%d.%m.%Y') }}|{{ document.added.isoformat() }}", "25.06.2020|2024-10-01T07:36:51.000153+00:00");
    r(
        "{{ correspondent|length }}|{{ correspondent|upper }}|{{ correspondent ~ '!' }}|{{ correspondent + '!' }}",
        "6|-NONE-|-none-!|-none-!",
    );
    r("{{ document.correspondent }}|{{ document.correspondent == none }}", "None|True");
    r("{{ 2 ** 3 ** 2 }}|{{ -2 ** 2 }}|{{ 2 ** 0.5 > 1 }}", "64|4|True");
    r("{{ 'ab' * 2 }}{{ [1] * 2 }}{{ 2 * 3 }}{{ 1 + 2.5 }}{{ [1] + [2] }}", "abab[1, 1]63.5[1, 2]");
    r("{% for t in document.tags %}{% if loop.index > 1 %}{% break %}{% endif %}{{ t.name }}{% endfor %}", "b");
    assert!(
        fails.is_empty(),
        "{}",
        fails.join(
            "
"
        )
    );
}

/// W9: Jinja2 filters/globals that were previously missing, and the `%` operator.
#[test]
fn test_w9_jinja2_extras() {
    let d = doc(7, "a b");
    let r = |f: &str| g(&d, &opts(Some(f)));
    assert_eq!(r("{{ '%s-%03d' % (title, document.pk) }}"), "a b-007.pdf");
    assert_eq!(r("{{ '%(x)s' % {'x': 'y'} }}"), "y.pdf");
    assert_eq!(r("{{ 7 % 3 }}{{ -7 % 3 }}"), "12.pdf");
    assert_eq!(r("{{ 'hello world foo' | wordwrap(5, wrapstring='_') }}"), "hello_world_foo.pdf");
    assert_eq!(r("{{ [1] | random }}"), "1.pdf");
    assert_eq!(r("{% set c = cycler('x', 'y') %}{{ c.next() }}{{ c.next() }}{{ c.next() }}{{ c.current }}"), "xyxy.pdf");
    assert_eq!(r("{% set j = joiner('-') %}{{ j() }}a{{ j() }}b"), "a-b.pdf");
    assert_eq!(r("{{ {'k': 'v', 'n': none} | xmlattr | trim }}"), "k=\"v\".pdf");
    assert!(r("{{ lipsum(1, false, 3, 5) }}").ends_with(".pdf"));
}
