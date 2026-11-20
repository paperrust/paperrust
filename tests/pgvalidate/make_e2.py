"""Generates experiments_e2.sql: tag list (alice) today (correlated document_count subquery)
vs. proposal (one GROUP BY over documents_document_tags, like upstream
annotate_document_count_by_ids). Input: results/pgvalidate/capture/alice_tags_count.sql"""
import sys
a = open(sys.argv[1], encoding="utf-8").read().strip()
start = a.index('"dt"."document_id" IN (') + len('"dt"."document_id" IN (')
depth, i = 1, start
while depth:
    depth += {"(": 1, ")": -1}.get(a[i], 0); i += 1
permitted = a[start:i - 1]
cs = a.index('(SELECT COUNT(DISTINCT "dt"."document_id")')
ce = a.index(' AS "document_count"')
where = a[a.index(' WHERE "o"."id" IN'):a.index(' ORDER BY')]
b = (a[:cs] + 'COALESCE("c"."n", 0)' + a[ce:a.index(' WHERE "o"."id" IN')]
     + ' LEFT JOIN (SELECT "dt"."tag_id" AS "k", COUNT(DISTINCT "dt"."document_id") AS "n" FROM "documents_document_tags" AS "dt" WHERE "dt"."document_id" IN ('
     + permitted + ') GROUP BY 1) AS "c" ON "c"."k" = "o"."id"' + where + ' ORDER BY LOWER(o.name) ASC')
out = ["SET auto_explain.log_min_duration = -1;", "SET statement_timeout = '900s';", r"\timing on",
       r"\echo '--- E2A heute (alice, tags.listAll)'", "EXPLAIN (ANALYZE, BUFFERS) " + a + ";",
       r"\echo '--- E2B Vorschlag GROUP BY'", "EXPLAIN (ANALYZE, BUFFERS) " + b + ";",
       r"\echo '--- E2 Ergebnisprüfung (0 Zeilen = gleich)'",
       "SELECT * FROM ((" + a.replace(" ORDER BY LOWER(o.name) ASC", "") + ") EXCEPT (" + b.replace(" ORDER BY LOWER(o.name) ASC", "") + ")) x;"]
open(sys.argv[2], "w", encoding="utf-8").write("\n".join(out) + "\n")
