-- PaperRust: Sequenzer v2 (docs/PERF-SCALE.md §5 #10d, §9). Crate: pl-sync.
--
-- Replaces only paperrust_sync_sequence(): with 1 million documents a call after a
-- 10,000-row bulk_edit took > 25 min, because the pending list was joined via LEFT JOIN with the UNION view
-- paperrust_sync_live (seq scan over documents_document per run), and it held
-- locks/snapshot so long that DROP DATABASE of other sessions in the cluster waited.
--
-- New: existence check per pending-list object by primary key (EXISTS per kind, only for the
-- touched IDs), smaller batches (default 5,000), lock_timeout on the function (never wait long for
-- locks; statement_timeout is set by the caller per transaction, pl_sync::sequence_once).
-- Result (feed entries, seq order, op, perm_seq, tombstones) is identical to v1.
--
-- * expand-only / idempotent: CREATE OR REPLACE of a paperrust_ function, no table.
-- * Rollback: apply migration 0001 again (restores v1).

CREATE OR REPLACE FUNCTION paperrust_sync_sequence(p_limit integer DEFAULT 5000) RETURNS integer
LANGUAGE plpgsql
SET lock_timeout = '5s'
AS $fn$
DECLARE
    v_xmin  xid8;
    n_ready integer;
BEGIN
    IF NOT pg_try_advisory_xact_lock(hashtext('paperrust_sync_sequencer')) THEN
        RETURN -1;
    END IF;
    -- own statement: the following statement has a newer snapshot in which all
    -- xid < v_xmin are finished (and thus, if committed, visible).
    v_xmin := pg_snapshot_xmin(pg_current_snapshot());

    WITH ready AS (
        DELETE FROM paperrust_sync_pending
        WHERE id IN (SELECT id FROM paperrust_sync_pending WHERE xid < v_xmin
                     ORDER BY id LIMIT p_limit)
        RETURNING kind, local_id, perm_change
    ), agg AS (
        SELECT kind, local_id, bool_or(perm_change) AS perm FROM ready GROUP BY kind, local_id
    ), ins AS (
        INSERT INTO paperrust_sync_log AS l (seq, kind, local_id, uuid, op, rev, perm_seq, at)
        SELECT q.seq, q.kind, q.local_id, q.uuid, q.op, q.rev, CASE WHEN q.perm THEN q.seq END, now()
        FROM (SELECT nextval('paperrust_sync_seq') AS seq, s.*
              FROM (SELECT a.kind, a.local_id, o.uuid, o.rev, a.perm,
                           -- same definition as the view paperrust_sync_live, but per row
                           -- via the primary key (local_id is bigint, id int4: the
                           -- int48 operator uses the PK index)
                           CASE WHEN CASE a.kind
                                    WHEN 1 THEN EXISTS (SELECT 1 FROM documents_document x
                                                        WHERE x.id = a.local_id AND x.deleted_at IS NULL
                                                          AND x.root_document_id IS NULL)
                                    WHEN 2 THEN EXISTS (SELECT 1 FROM documents_tag x WHERE x.id = a.local_id)
                                    WHEN 3 THEN EXISTS (SELECT 1 FROM documents_correspondent x WHERE x.id = a.local_id)
                                    WHEN 4 THEN EXISTS (SELECT 1 FROM documents_documenttype x WHERE x.id = a.local_id)
                                    WHEN 5 THEN EXISTS (SELECT 1 FROM documents_storagepath x WHERE x.id = a.local_id)
                                    WHEN 6 THEN EXISTS (SELECT 1 FROM documents_customfield x WHERE x.id = a.local_id)
                                    WHEN 7 THEN EXISTS (SELECT 1 FROM documents_savedview x WHERE x.id = a.local_id)
                                    WHEN 8 THEN EXISTS (SELECT 1 FROM auth_user x WHERE x.id = a.local_id)
                                    WHEN 9 THEN EXISTS (SELECT 1 FROM auth_group x WHERE x.id = a.local_id)
                                    ELSE false END
                                THEN 'u' ELSE 'd' END AS op
                    FROM agg a
                    JOIN paperrust_sync_object o ON o.kind = a.kind AND o.local_id = a.local_id
                    ORDER BY a.kind, a.local_id) s) q
        ON CONFLICT (kind, local_id) DO UPDATE
            SET seq = EXCLUDED.seq, uuid = EXCLUDED.uuid, op = EXCLUDED.op, rev = EXCLUDED.rev,
                -- an earlier permission change is kept (client with an older cursor)
                perm_seq = coalesce(EXCLUDED.perm_seq, l.perm_seq), at = EXCLUDED.at
        RETURNING l.kind, l.local_id, l.seq, l.op
    ), upd AS (
        UPDATE paperrust_sync_object o
           SET seq = x.seq,
               deleted_at = CASE WHEN x.op = 'd' THEN coalesce(o.deleted_at, now()) END
          FROM (SELECT o2.kind, o2.local_id, i.seq, i.op
                  FROM paperrust_sync_object o2
                  JOIN ins i ON i.kind = o2.kind AND i.local_id = o2.local_id
                 ORDER BY o2.kind, o2.local_id
                 -- never wait for writers (no deadlock with Django/Rust transactions);
                 -- a locked row has a pending-list entry itself and comes up in the
                 -- next run
                   FOR UPDATE OF o2 SKIP LOCKED) x
         WHERE o.kind = x.kind AND o.local_id = x.local_id
    )
    SELECT count(*) INTO n_ready FROM ready;
    IF n_ready > 0 THEN
        PERFORM pg_notify('paperrust_sync', '');
    END IF;
    RETURN n_ready;
END
$fn$;
