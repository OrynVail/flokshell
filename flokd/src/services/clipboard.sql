CREATE TABLE IF NOT EXISTS clipboard_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash TEXT NOT NULL UNIQUE,
    mime_type TEXT NOT NULL DEFAULT 'text/plain',
    preview TEXT NOT NULL,
    full_content TEXT,
    is_image INTEGER NOT NULL DEFAULT 0,
    binary_path TEXT,
    size INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    alias TEXT,
    display_index INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_content_hash ON clipboard_items(content_hash);
CREATE INDEX IF NOT EXISTS idx_created_at ON clipboard_items(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_is_image ON clipboard_items(is_image);
CREATE INDEX IF NOT EXISTS idx_pinned ON clipboard_items(pinned DESC);
CREATE INDEX IF NOT EXISTS idx_display_index ON clipboard_items(pinned DESC, display_index ASC);
