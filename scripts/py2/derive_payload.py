# -*- coding: utf-8 -*-
"""FSD derive payload — runs under the Python the client's loaders are
built against (2.7 today). Written in the 2/3-compatible source subset
so a py3 client only changes the interpreter bootstrap, never this
script: import the official <name>Loader.pyd decoders from bin64, join
types x iconids x groups x categories with the localization pickle and
emit derived.json (utf-8).
"""
import argparse
import codecs
import json
import sys

if sys.version_info[0] >= 3:
    text_type = str
else:
    text_type = unicode  # noqa: F821  (py2 name)


def norm_icon(path):
    p = path.replace("\\", "/").lower()
    if not p.startswith("res:"):
        last = p.rsplit("/", 1)[-1]
        if "." not in last:
            p = "res:/ui/texture/icons/" + p + ".png"
    return p


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin64", required=True)
    ap.add_argument("--types", required=True)
    ap.add_argument("--iconids", required=True)
    ap.add_argument("--groups", required=True)
    ap.add_argument("--categories", required=True)
    ap.add_argument("--localization", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    sys.path.insert(0, a.bin64)
    import typesLoader
    import iconIDsLoader
    import groupsLoader
    import categoriesLoader

    types = typesLoader.load(a.types)
    icons = iconIDsLoader.load(a.iconids)
    groups = groupsLoader.load(a.groups)
    cats = categoriesLoader.load(a.categories)
    print("loaded: types=%d iconids=%d groups=%d categories=%d"
          % (len(types), len(icons), len(groups), len(cats)))

    import pickle
    with open(a.localization, "rb") as f:
        lang, table = pickle.load(f)
    print("localization: %s, %d messages" % (lang, len(table)))

    def zh(msg_id):
        if msg_id in (None, 0):
            return None
        entry = table.get(int(msg_id))
        if entry and entry[0]:
            return text_type(entry[0])
        return None

    def icon_file(icon_id):
        if not icon_id:
            return None
        rec = icons.get(icon_id)
        if rec is None:
            return None
        f = getattr(rec, "iconFile", None)
        return text_type(f) if f else None

    def icon_of(rec):
        f = icon_file(getattr(rec, "iconID", None))
        if f:
            return f
        g = groups.get(getattr(rec, "groupID", None))
        if g is not None:
            f = icon_file(getattr(g, "iconID", None))
            if f:
                return f
            c = cats.get(getattr(g, "categoryID", None))
            if c is not None:
                f = icon_file(getattr(c, "iconID", None))
                if f:
                    return f
        return None

    out = {}
    missing_name = 0
    no_icon = 0
    for tid in types.keys():
        rec = types[tid]
        name = zh(getattr(rec, "typeNameID", None))
        if name is None:
            missing_name += 1
            continue
        icon = icon_of(rec)
        if icon is None:
            no_icon += 1
        out[int(tid)] = {"name": name, "icon": norm_icon(icon) if icon else None}

    meta = {
        "type_records": len(types),
        "derived_types": len(out),
        "missing_name": missing_name,
        "missing_icon": no_icon,
        "localization_messages": len(table),
    }
    with codecs.open(a.out, "w", "utf-8") as f:
        json.dump({"meta": meta, "types": {str(k): v for k, v in out.items()}},
                  f, ensure_ascii=False)
    print("derived=%d missing_name=%d missing_icon=%d"
          % (len(out), missing_name, no_icon))

    # anchors (fail loudly if the client data stops matching shape)
    assert out.get(34, {}).get("name"), "Tritanium anchor missing"
    assert out.get(34, {}).get("icon"), "Tritanium icon missing"
    assert out.get(645), "Dominix anchor missing"


if __name__ == "__main__":
    main()
