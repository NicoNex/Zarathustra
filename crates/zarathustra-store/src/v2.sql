-- Schema v2: the app's preferences, one row per group (settings, typography, home view,
-- reading pace), each the group's own `key=value` text. Boot-time switches that shell scripts
-- read stay in a text file outside the database.
CREATE TABLE prefs(
  name TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
