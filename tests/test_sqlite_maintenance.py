import re
import sqlite3
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "db" / "schema" / "sqlite"
SOURCE = (ROOT / "db" / "src" / "sqlite.rs").read_text(encoding="utf-8")


@pytest.fixture
def database():
    connection = sqlite3.connect(":memory:")
    connection.executescript((SCHEMA / "01_sqlite.sql").read_text(encoding="utf-8"))
    connection.executescript(
        (SCHEMA / "02_lost_friends.sql").read_text(encoding="utf-8")
    )
    yield connection
    connection.close()


def migrate(connection):
    connection.executescript(
        (SCHEMA / "03_post_identity_indexes.sql").read_text(encoding="utf-8")
    )


def test_migration_keeps_latest_copy_after_author_rename(database):
    rows = [
        ("old", "https://example.com/post", "Rain", "2026-10-01"),
        ("new", "https://example.com/post", "New name", "2026-10-20"),
        ("latest", "https://example.com/post", "New name", "2026-10-20"),
        ("unrelated", "https://example.com/other", "Other", "2026-10-01"),
    ]
    database.executemany(
        "INSERT INTO posts(title,link,author,createdAt) VALUES(?,?,?,?)", rows
    )
    migrate(database)
    assert database.execute("SELECT title FROM posts ORDER BY id").fetchall() == [
        ("latest",),
        ("unrelated",),
    ]
    with pytest.raises(sqlite3.IntegrityError):
        database.execute("INSERT INTO posts(link) VALUES('https://example.com/post')")


def test_single_upsert_updates_author_without_deleting_row(database):
    migrate(database)
    sql = re.search(r'let sql = "(INSERT INTO posts.*?)";', SOURCE, re.S).group(1)
    original = (
        "Post",
        "Rain",
        "https://example.com/post",
        "old.png",
        "feed",
        "2026-10-01",
        "2026-10-01",
        "2026-10-01",
    )
    database.execute(sql, original)
    original_id = database.execute("SELECT id FROM posts").fetchone()[0]
    refreshed = list(original)
    refreshed[1] = "New name"
    refreshed[3] = "new.png"
    refreshed[6] = "2026-10-20"
    database.execute(sql, refreshed)
    assert database.execute(
        "SELECT id,author,avatar,created,updated FROM posts"
    ).fetchall() == [(original_id, "New name", "new.png", "2026-10-01", "2026-10-20")]


def test_bulk_upsert_handles_duplicate_urls_in_same_batch(database):
    migrate(database)
    bulk_source = SOURCE.split("pub async fn bulk_insert_post_table", 1)[1]
    clause = re.search(
        r'query_builder.push\(\s*"(.*?)",\s*\);', bulk_source, re.S
    ).group(1)
    sql = "INSERT INTO posts(link,author) VALUES (?,?),(?,?)" + clause
    database.execute(
        sql,
        ("https://example.com/post", "Rain", "https://example.com/post", "New name"),
    )
    assert database.execute("SELECT link,author FROM posts").fetchall() == [
        ("https://example.com/post", "New name")
    ]


def test_date_sorting_uses_indexes(database):
    migrate(database)
    for column in ("created", "updated"):
        plan = str(
            database.execute(
                f"EXPLAIN QUERY PLAN SELECT * FROM posts ORDER BY {column} DESC LIMIT 30"
            ).fetchall()
        )
        assert f"posts_{column}_idx" in plan
        assert "TEMP B-TREE" not in plan


def test_orphan_summary_cleanup_keeps_live_article_summary(database):
    migrate(database)
    database.execute("INSERT INTO posts(link) VALUES('https://example.com/live')")
    database.executemany(
        "INSERT INTO article_summaries(link,content_hash) VALUES(?,?)",
        [("https://example.com/live", "a"), ("https://example.com/deleted", "b")],
    )
    sql = re.search(
        r'query\("(DELETE FROM article_summaries WHERE NOT EXISTS.*?)"\)', SOURCE, re.S
    ).group(1)
    database.execute(sql)
    assert database.execute("SELECT link FROM article_summaries").fetchall() == [
        ("https://example.com/live",)
    ]


def test_actions_schedule_is_twice_daily_in_shanghai():
    workflow = yaml.load(
        (ROOT / ".github" / "workflows" / "CI.yml").read_text(encoding="utf-8"),
        Loader=yaml.BaseLoader,
    )
    assert workflow["on"]["schedule"] == [{"cron": "0 0,12 * * *"}]
    settings = yaml.safe_load((ROOT / "fc_settings.yaml").read_text(encoding="utf-8"))
    assert settings["CRON"] == "0 8,20 * * *"
    concurrency = workflow["jobs"]["build-and-run-core"]["concurrency"]
    assert concurrency["cancel-in-progress"] == "false"
