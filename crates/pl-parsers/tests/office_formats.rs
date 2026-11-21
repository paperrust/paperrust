//! Gap E13: XLSX cell formats and office metadata like Tika.

use pl_parsers::office::{OfficeDocumentParser, extract};
use pl_parsers::{DocumentParser, ParseContext};

const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Uncompressed ZIP without CRC (the reader does not check CRCs).
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in entries {
        let off = v.len() as u32;
        v.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(data);
        cd.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0u8; 12]);
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = v.len() as u32;
    let n = entries.len() as u16;
    v.extend_from_slice(&cd);
    v.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    v.extend_from_slice(&cd_off.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v
}

const CT: &[u8] = br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#;
const ROOT_RELS: &[u8] = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties" Target="docProps/custom.xml"/>
</Relationships>"#;
const CORE: &[u8] = br#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/">
<dc:title>Liste</dc:title><dc:identifier>ID-7</dc:identifier><cp:version>2.1</cp:version><dcterms:created>2024-03-15T10:00:00Z</dcterms:created></cp:coreProperties>"#;
const APP: &[u8] = br#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
<Application>Microsoft Excel</Application><DocSecurity>0</DocSecurity><Company>Muster GmbH</Company><Pages>0</Pages><CharactersWithSpaces>120</CharactersWithSpaces></Properties>"#;
const CUSTOM: &[u8] = r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
<property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="Projekt"><vt:lpwstr>Umbau</vt:lpwstr></property>
<property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="Geprüft"><vt:bool>true</vt:bool></property></Properties>"#
    .as_bytes();

fn xlsx(date1904: bool) -> Vec<u8> {
    let wb = format!(
        r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr date1904="{}"/><sheets><sheet name="Daten" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        u8::from(date1904)
    );
    let wb_rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#;
    let styles = br#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<numFmts count="1"><numFmt numFmtId="164" formatCode="dd.mm.yyyy"/></numFmts>
<cellStyleXfs count="1"><xf numFmtId="0"/></cellStyleXfs>
<cellXfs count="5"><xf numFmtId="0"/><xf numFmtId="164" applyNumberFormat="1"/><xf numFmtId="14"/><xf numFmtId="10"/><xf numFmtId="4"/></cellXfs></styleSheet>"#;
    let sst = b"<?xml version=\"1.0\"?><sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><si><t>Zeile 1\nZeile 2</t></si></sst>";
    let sheet = br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
<row r="1"><c r="A1" s="1"><v>45366</v></c><c r="B1" s="2"><v>45366</v></c><c r="C1" s="3"><v>0.256</v></c><c r="D1" s="4"><v>1234.5</v></c><c r="E1"><v>7.25</v></c><c r="F1" t="s"><v>0</v></c></row>
</sheetData></worksheet>"#;
    stored_zip(&[
        ("[Content_Types].xml", CT),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels),
        ("xl/styles.xml", styles),
        ("xl/sharedStrings.xml", sst),
        ("xl/worksheets/sheet1.xml", sheet),
        ("docProps/core.xml", CORE),
        ("docProps/app.xml", APP),
        ("docProps/custom.xml", CUSTOM),
    ])
}

#[test]
fn xlsx_cells_use_their_number_format() {
    let ctx = ParseContext::default();
    let c = extract(&xlsx(false), XLSX, &ctx).unwrap();
    // Date (custom format, built-in 14), percent, thousands, General; line break in the cell is kept
    assert_eq!(c.text, "Daten\n\t15.03.2024\t3/15/24\t25.60%\t1,234.50\t7.25\tZeile 1\nZeile 2\n\n");
    // 1904 date system: 4 years and 1 day later
    let c = extract(&xlsx(true), XLSX, &ctx).unwrap();
    assert!(c.text.starts_with("Daten\n\t16.03.2028\t3/16/28\t"), "{}", c.text);
}

#[test]
fn ooxml_metadata_like_tika() {
    let ctx = ParseContext::default();
    let c = extract(&xlsx(false), XLSX, &ctx).unwrap();
    let get = |k: &str| c.metadata.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    assert_eq!(get("dc:title"), Some("Liste"));
    assert_eq!(get("dc:identifier"), Some("ID-7"));
    assert_eq!(get("cp:version"), Some("2.1"));
    assert_eq!(get("extended-properties:Company"), Some("Muster GmbH"));
    assert_eq!(get("dc:publisher"), Some("Muster GmbH"));
    assert_eq!(get("extended-properties:DocSecurity"), Some("0"));
    assert_eq!(get("extended-properties:DocSecurityString"), Some("None"));
    assert_eq!(get("meta:character-count-with-spaces"), Some("120"));
    // Tika omits a counter of 0
    assert_eq!(get("meta:page-count"), None);
    assert_eq!(get("xmpTPg:NPages"), None);
    assert_eq!(get("custom:Projekt"), Some("Umbau"));
    assert_eq!(get("custom:Geprüft"), Some("true"));
    assert_eq!(get("Content-Type"), Some(XLSX));
    assert_eq!(c.created.as_deref(), Some("2024-03-15T10:00:00Z"));
}

#[test]
fn docx_line_breaks_and_metadata_through_parser() {
    let rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#;
    let doc = br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:r><w:t>Erste</w:t><w:br/><w:t>Zweite</w:t><w:tab/><w:t>Tab</w:t></w:r></w:p><w:p><w:r><w:t>Absatz</w:t></w:r></w:p></w:body></w:document>"#;
    let app = br#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Pages>2</Pages><Words>3</Words><DocSecurity>4</DocSecurity></Properties>"#;
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.docx");
    std::fs::write(
        &p,
        stored_zip(&[("[Content_Types].xml", CT), ("_rels/.rels", rels), ("word/document.xml", doc), ("docProps/app.xml", app)]),
    )
    .unwrap();
    let parser = OfficeDocumentParser::default();
    let ctx = ParseContext::default();
    let d = parser.parse(&p, DOCX, &ctx).unwrap();
    assert_eq!(d.text, "Erste\nZweite\tTab\nAbsatz");
    let md = parser.extract_metadata(&p, DOCX, &ctx);
    let get = |k: &str| md.iter().find(|m| m.key == k).map(|m| m.value.clone());
    assert_eq!(get("meta:page-count").as_deref(), Some("2"));
    assert_eq!(get("xmpTPg:NPages").as_deref(), Some("2"));
    assert_eq!(get("meta:word-count").as_deref(), Some("3"));
    assert_eq!(get("extended-properties:DocSecurityString").as_deref(), Some("ReadOnlyEnforced"));
    assert!(md.iter().all(|m| m.namespace.is_empty() && m.prefix.is_empty()));
}
