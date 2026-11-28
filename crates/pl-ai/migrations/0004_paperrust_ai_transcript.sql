-- PaperRust: transcripts of the AI text recognition ("KI-Abschrift", docs/OCR-NEXT.md 2.7/2.8).
-- Expand-only, idempotent (IF NOT EXISTS). Rollback:
--   DROP TABLE paperrust_ai_transcript;  (no dependencies on upstream except the document FK)

BEGIN;

-- One row per page (line IS NULL) or per line crop of the latest AI run on a document. Keeps the
-- original OCR text (revert/compare) next to the AI text; rows of an earlier run are replaced.
-- status: accepted | loop | token_limit | too_long | empty | skipped
CREATE TABLE IF NOT EXISTS paperrust_ai_transcript (
    id          bigserial     PRIMARY KEY,
    document_id integer       NOT NULL,
    created     timestamptz   NOT NULL DEFAULT now(),
    model       varchar(128)  NOT NULL,
    mode        varchar(16)   NOT NULL,
    page        integer       NOT NULL CHECK (page >= 1),
    line        integer       NULL CHECK (line IS NULL OR line >= 0),
    bbox        jsonb         NULL,
    ocr_text    text          NOT NULL,
    ai_text     text          NOT NULL,
    status      varchar(12)   NOT NULL
                CHECK (status IN ('accepted', 'loop', 'token_limit', 'too_long', 'empty', 'skipped'))
);

-- Foreign key to the real document table: after the data model conversion (pl_store dm1,
-- docs/DATAMODEL-V2.md) `documents_document` is a compatibility view, the rows live in
-- `paperrust_document`. Before the conversion dm1 re-points incoming keys itself.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'paperrust_ai_transcript_document_fk') THEN
        IF to_regclass('public.paperrust_document') IS NOT NULL THEN
            ALTER TABLE paperrust_ai_transcript ADD CONSTRAINT paperrust_ai_transcript_document_fk
                FOREIGN KEY (document_id) REFERENCES paperrust_document (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        ELSE
            ALTER TABLE paperrust_ai_transcript ADD CONSTRAINT paperrust_ai_transcript_document_fk
                FOREIGN KEY (document_id) REFERENCES documents_document (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        END IF;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS paperrust_ai_transcript_document ON paperrust_ai_transcript (document_id, page);

COMMIT;
