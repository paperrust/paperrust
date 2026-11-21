//! OCR language automatic (`PAPERRUST_OCR_LANGUAGE_AUTO`): decision when to recognize a document a
//! second time with its detected language (pl-consumer `native::auto_ocr_language`).

use pl_consumer::native::{AUTO_LANGUAGE_MIN_LETTERS, auto_ocr_language, langdetect_to_tesseract};

const FR: &str = "Nous vous remercions de votre commande et vous prions de régler la facture dans les trente jours. \
Pour toute question concernant cette facture, nous sommes à votre disposition du lundi au vendredi. \
La marchandise reste notre propriété jusqu'au paiement intégral. Veuillez agréer, Madame, Monsieur, \
l'expression de nos salutations distinguées. Entretien annuel du chauffage et réparation de fenêtre.";
const DE: &str = "Wir danken Ihnen für Ihren Auftrag und bitten um Überweisung innerhalb von dreissig Tagen. \
Bei Fragen zu dieser Rechnung erreichen Sie uns werktags von acht bis siebzehn Uhr. Wir weisen darauf hin, \
dass die Ware bis zur vollständigen Bezahlung unser Eigentum bleibt. Für Rückfragen stehen wir Ihnen gerne \
zur Verfügung. Wartung der Heizungsanlage und Reparatur der Fenster im Treppenhaus.";

fn langs(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

#[test]
fn french_document_with_german_english_setting_is_recognized_again_in_french() {
    assert_eq!(auto_ocr_language(FR, "deu+eng", &langs(&["deu", "eng", "fra", "osd"])), Some("fra"));
}

#[test]
fn configured_language_needs_no_second_run() {
    assert_eq!(auto_ocr_language(DE, "deu+eng", &langs(&["deu", "eng", "fra"])), None);
    assert_eq!(auto_ocr_language(FR, "fra+eng", &langs(&["deu", "eng", "fra"])), None);
}

#[test]
fn missing_language_data_means_no_second_run() {
    assert_eq!(auto_ocr_language(FR, "deu+eng", &langs(&["deu", "eng"])), None);
}

#[test]
fn short_text_is_not_trusted() {
    let short: String = FR.chars().take(AUTO_LANGUAGE_MIN_LETTERS / 2).collect();
    assert_eq!(auto_ocr_language(&short, "deu+eng", &langs(&["deu", "eng", "fra"])), None);
    assert_eq!(auto_ocr_language("1234 5678 9012 CHF 45.00 12.11.2026 0041 44 123 45 67", "deu", &langs(&["eng"])), None);
}

#[test]
fn language_codes_map_to_tesseract() {
    assert_eq!(langdetect_to_tesseract("fr"), Some("fra"));
    assert_eq!(langdetect_to_tesseract("de"), Some("deu"));
    assert_eq!(langdetect_to_tesseract("unknown"), None);
}
