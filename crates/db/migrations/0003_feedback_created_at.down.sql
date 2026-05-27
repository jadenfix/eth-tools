-- 0003_feedback_created_at — down.
DROP INDEX IF EXISTS idx_feedback_created_at;
ALTER TABLE feedback DROP COLUMN IF EXISTS created_at;
