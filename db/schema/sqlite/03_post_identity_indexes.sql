-- A blog rename must not create another copy of the same article URL.
-- Keep the most recently crawled copy, breaking timestamp ties with the row ID.
DELETE FROM posts
WHERE id IN (
    SELECT id FROM (
        SELECT id, ROW_NUMBER() OVER (
            PARTITION BY link ORDER BY "createdAt" DESC, id DESC
        ) AS duplicate_rank
        FROM posts
        WHERE link IS NOT NULL
    )
    WHERE duplicate_rank > 1
);

CREATE UNIQUE INDEX posts_link_unique ON posts (link);
CREATE INDEX posts_created_idx ON posts (created DESC);
CREATE INDEX posts_updated_idx ON posts (updated DESC);
CREATE INDEX friends_link_idx ON friends (link);
