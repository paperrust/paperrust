// upstream: src/documents/views.py@v3.4.2 (SharedLinkView: `_("The share link bundle is still being
//           prepared. Please try again later.")`, `_("The share link bundle is unavailable.")`),
//           src/locale/*/LC_MESSAGES/django.po@v3.4.2 (non-fuzzy translations, generated)
//
//! Translations of the two share-link-bundle texts for the active language
//! (`pl_core::locale::language_from_headers`, Django language code). Languages without a
//! translation (or with an empty one) get the English text like Django's gettext.

/// `(language, bundle still being prepared, bundle unavailable)`; `""` = not translated.
const CATALOG: &[(&str, &str, &str)] = &[
    ("ca-es", "El paquet de link encarà s'està preparant. Prova de nou més tard.", "El paquet d'enllaç no està disponible."),
    ("de-ch", "Das Freigabelink-Paket wird noch vorbereitet. Bitte versuchen Sie es später erneut.", "Das Freigabelink-Paket ist nicht verfügbar."),
    ("de-de", "Das Freigabelink-Paket wird noch vorbereitet. Bitte versuchen Sie es später erneut.", "Das Freigabelink-Paket ist nicht verfügbar."),
    ("es-es", "El paquete de enlace compartido aún está siendo preparado. Por favor, inténtalo de nuevo más tarde.", "El paquete de enlace compartido no está disponible."),
    ("fr-fr", "Le lot de liens de partage est en cours de préparation. Veuillez réessayer plus tard.", "Le lot de liens de partage n'est pas disponible."),
    ("he-il", "חבילת קישור השיתוף עדיין בהכנה. נא לנסות שוב מאוחר יותר.", "חבילת קישור השיתוף אינה זמינה."),
    ("hr-hr", "Paket linka za dijeljenje se još priprema. Pokušajte ponovo kasnije.", "Paket linka za dijeljenje nije dostupan."),
    ("hu-hu", "A megosztási linkcsomag készítése folyamatban. Kérjük, próbálja meg később.", "A megosztási linkcsomag nem elérhető."),
    ("id-id", "Bundel tautan berbagi masih dalam proses persiapan. Silakan coba lagi nanti.", "Bundel tautan berbagi tidak tersedia."),
    ("it-it", "Il pacchetto di link di condivisione è ancora in fase di preparazione. Riprova più tardi.", "Il pacchetto di link di condivisione non è disponibile."),
    ("lt-lt", "Bendrinimo nuorodų rinkinys vis dar ruošiamas. Bandykite dar kartą vėliau.", "Dalinimos nuorodų rinkinys yra nepasiekiamas."),
    ("mk-mk", "Пакетот врски за споделување сè уште се подготвува. Обидете се повторно подоцна.", "Пакетот врски за споделување не е достапен."),
    ("no-no", "The share link bundle is still being prepared. Please try again later. (NO)", "The share link bundle is unavailable. (NO)"),
    ("pl-pl", "Pakiet do udostępnienia jest nadal przygotowywany. Spróbuj ponownie później.", "Pakiet do udostępnienia jest niedostępny."),
    ("pt-br", "O pacote de link de compartilhamento ainda está sendo preparado. Tente novamente mais tarde.", "O pacote de link de compartilhamento não está disponível."),
    ("ru-ru", "Набор ссылок все еще готовится. Пожалуйста, повторите попытку позже.", "Пакет общих ссылок недоступен."),
    ("sk-sk", "Balík odkazov na zdieľanie sa stále pripravuje. Skúste to znova neskôr.", "Balík odkazov na zdieľanie nie je k dispozícii."),
    ("sl-si", "Paket povezav za deljenje je še vedno v pripravi. Poskusite znova pozneje.", "Paket povezav za deljenje ni na voljo."),
    ("sq-al", "Paketa e lidhjes së ndarjes ende po përgatitet. Ju lutemi provoni përsëri më vonë.", "Paketa e lidhjes së ndarjes nuk është e disponueshme."),
    ("sr-cs", "Paket linkova za deljenje se još uvek priprema. Molimo pokušajte ponovo kasnije.", "Paket linkova za deljenje nije dostupan."),
    ("sv-se", "Delningslänkpaketet förbereds fortfarande. Försök igen senare.", "Delningslänkpaketet är inte tillgängligt."),
    ("vi-vn", "Liên kết chia sẻ gói vẫn đang được chuẩn bị. Vui lòng thử lại sau.", "Không tồn tại liên kết chia sẻ gói"),
    ("zh-cn", "分享链接包仍在准备中，请稍后再试。", "分享链接包不可用。"),
    ("zh-tw", "共用連結組合仍在準備中，請稍後再試。", "共用連結組合無法使用。"),
];

pub const PREPARING: &str = "The share link bundle is still being prepared. Please try again later.";
pub const UNAVAILABLE: &str = "The share link bundle is unavailable.";

fn row(lang: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    CATALOG.iter().find(|(l, _, _)| *l == lang)
}

/// `_("The share link bundle is still being prepared. Please try again later.")`.
pub fn preparing(lang: &str) -> &'static str {
    row(lang).map(|r| r.1).filter(|t| !t.is_empty()).unwrap_or(PREPARING)
}

/// `_("The share link bundle is unavailable.")`.
pub fn unavailable(lang: &str) -> &'static str {
    row(lang).map(|r| r.2).filter(|t| !t.is_empty()).unwrap_or(UNAVAILABLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gettext_like_django() {
        assert_eq!(preparing("en-us"), PREPARING);
        assert_eq!(unavailable("de-de"), "Das Freigabelink-Paket ist nicht verfügbar.");
        assert_eq!(preparing("fr-fr"), "Le lot de liens de partage est en cours de préparation. Veuillez réessayer plus tard.");
        assert_eq!(unavailable("xx-xx"), UNAVAILABLE);
    }
}
