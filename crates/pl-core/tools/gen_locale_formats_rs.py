import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
out = []
out.append("//! Generated from django/conf/locale/*/formats.py@5.2.18 (DATE_INPUT_FORMATS, DATETIME_INPUT_FORMATS)\n//! for the languages from paperless `settings.LANGUAGES`, incl. the `formats.get_format` logic\n//! (locale module `xx_YY`, then `xx`, otherwise global settings; ISO formats appended).\n//! Do not edit by hand – for the generator see docs/contract/pl-core.md.\n")
out.append("/// (Sprachcode, DATE_INPUT_FORMATS, DATETIME_INPUT_FORMATS)")
out.append("pub(crate) static LOCALE_INPUT_FORMATS: &[(&str, &[&str], &[&str])] = &[")
def lit(s):
    return json.dumps(s, ensure_ascii=False)
for lang, e in d.items():
    out.append(f"    ({lit(lang)}, &[{', '.join(lit(x) for x in e['DATE_INPUT_FORMATS'])}], &[{', '.join(lit(x) for x in e['DATETIME_INPUT_FORMATS'])}]),")
out.append("];\n")
out.append("/// Sprachcodes aus paperless `settings.LANGUAGES` in Reihenfolge (erstes = Fallback `en-us`).")
out.append("pub(crate) static LANGUAGES: &[&str] = &[" + ", ".join(lit(l) for l in d.keys()) + "];")
open(sys.argv[2], "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
