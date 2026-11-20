"""Erzeugt formats.json aus django/conf/locale (Django 5.2.18, ohne Import: ast.literal_eval).
Aufruf: python -I gen_locale_formats.py <django/conf/locale> > formats.json
danach: python -I gen_locale_formats_rs.py formats.json ../src/locale_formats.rs"""
import ast, sys, os, json
loc = sys.argv[1]
langs = ["en-us","ar-ar","af-za","be-by","bg-bg","ca-es","cs-cz","da-dk","de-de","el-gr","en-gb","es-es","fa-ir","fi-fi","fr-fr","hu-hu","id-id","it-it","ja-jp","ko-kr","lb-lu","no-no","nl-nl","pl-pl","pt-br","pt-pt","ro-ro","ru-ru","sk-sk","sl-si","sr-cs","sv-se","tr-tr","uk-ua","vi-vn","zh-cn","zh-tw"]
def to_locale(language):
    language, _, country = language.lower().partition("-")
    if not country:
        return language[:3].lower() + language[3:]
    country, _, tail = country.partition("-")
    country = country.title() if len(country) > 2 else country.upper()
    if tail:
        country += "-" + tail
    return language + "_" + country
def read(mod):
    p = os.path.join(loc, mod, "formats.py")
    if not os.path.exists(p): return {}
    t = ast.parse(open(p, encoding="utf-8").read())
    out = {}
    for n in t.body:
        if isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Name):
            try: out[n.targets[0].id] = ast.literal_eval(n.value)
            except Exception: pass
    return out
glob = {"DATE_INPUT_FORMATS": ["%Y-%m-%d","%m/%d/%Y","%m/%d/%y","%b %d %Y","%b %d, %Y","%d %b %Y","%d %b, %Y","%B %d %Y","%B %d, %Y","%d %B %Y","%d %B, %Y"],
 "DATETIME_INPUT_FORMATS": ["%Y-%m-%d %H:%M:%S","%Y-%m-%d %H:%M:%S.%f","%Y-%m-%d %H:%M","%m/%d/%Y %H:%M:%S","%m/%d/%Y %H:%M:%S.%f","%m/%d/%Y %H:%M","%m/%d/%y %H:%M:%S","%m/%d/%y %H:%M:%S.%f","%m/%d/%y %H:%M"]}
iso = {"DATE_INPUT_FORMATS": ["%Y-%m-%d"], "DATETIME_INPUT_FORMATS": ["%Y-%m-%d %H:%M:%S","%Y-%m-%d %H:%M:%S.%f","%Y-%m-%d %H:%M","%Y-%m-%d"]}
res = {}
for l in langs:
    lc = to_locale(l)
    mods = [lc] + ([lc.split("_")[0]] if "_" in lc else [])
    entry = {}
    for ft in ["DATE_INPUT_FORMATS","DATETIME_INPUT_FORMATS"]:
        val = None
        for m in mods:
            d = read(m)
            if ft in d: val = list(d[ft]); src=m; break
        if val is None:
            val = list(glob[ft]); src="global"
        else:
            for i in iso[ft]:
                if i not in val: val.append(i)
        entry[ft] = val
    res[l] = entry
print(json.dumps(res))
