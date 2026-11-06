# SQLite in Go for Roastro

Research for issue #3. Checked on 2026-11-06 against the Go module proxy, pkg.go.dev, the drivers' own docs, and sqlite.org.

## Question

Which SQLite driver should Roastro use, and how should schema migrations run? Constraints:

- One binary, cross-compiled from Linux to Windows.
- About one sample per second written during a roast.
- The code should be simple, idiomatic Go that a learner can read.
- Use the standard `database/sql` package.

## Recommendation

1. **Driver:** `modernc.org/sqlite`, used through `database/sql` with driver name `"sqlite"`. It needs no cgo, so `GOOS=windows go build` works from Linux with no C toolchain.
2. **Migrations:** numbered `.sql` files embedded with `//go:embed`, plus a small homegrown runner of about 40 lines. The runner tracks the schema version in `PRAGMA user_version` and applies each file in its own transaction. Do not add goose or golang-migrate yet.
3. **Connection setup:**
   - Set `journal_mode(WAL)`, `synchronous(NORMAL)`, `busy_timeout(5000)` and `foreign_keys(1)` in the DSN, so every pooled connection gets them.
   - Keep the database in a per-user app-data directory built with `filepath`, not in the working directory or on a network share.

## 1. Driver

### Candidates (versions from proxy.golang.org)

| Module | Latest | Released | How it works | cgo | `database/sql` name |
|---|---|---|---|---|---|
| `modernc.org/sqlite` | v1.61.1 | 2026-10-31 | SQLite C code transpiled to Go (SQLite 3.54.1) | no | `"sqlite"` |
| `github.com/ncruces/go-sqlite3` | v0.36.0 | 2026-10-29 | SQLite built to Wasm, translated to Go with wasm2go | no | `"sqlite3"` (import `.../driver`) |
| `github.com/mattn/go-sqlite3` | v1.14.53 | 2026-11-05 | cgo bindings to the C amalgamation | **yes** | `"sqlite3"` |

All three released within the last month, so all are actively maintained.

### Why not mattn/go-sqlite3

The mattn README says it "is a cgo package": you must set `CGO_ENABLED=1` and have gcc on the PATH. Cross-compiling needs a `CC` cross compiler, which for Windows means mingw-w64. That adds a C toolchain to every Windows build for no gain at 1 write per second. It is the most battle-tested driver, but its main advantage is raw speed, and Roastro does not need that. Source: https://github.com/mattn/go-sqlite3

### modernc vs ncruces

Both are cgo-free and both support Windows.

**modernc.org/sqlite**
- Calls itself a "CGo-free port" of SQLite and publishes builds for windows/amd64, windows/arm64 and windows/386.
- Its docs measure 1.3x to 2.0x the CPU time of C on CPU-bound work. That cost does not matter at 1 Hz inserts.
- The DSN accepts `_pragma=name(value)` parameters, applied to every new connection.
- golang-migrate's `database/sqlite` driver is built on it, which shows other projects depend on it.
- Pin the `modernc.org/libc` version the package asks for in go.mod. Its docs tie their results to a specific libc version, and a mismatched libc is the classic way to break this driver.
- Source: https://pkg.go.dev/modernc.org/sqlite

**github.com/ncruces/go-sqlite3**
- The project tests every commit on windows/amd64 and windows/arm64. Its support matrix lists full file locking and shared-memory WAL on Windows.
- It also offers its own non-`database/sql` API.
- It is still v0.x, so breaking API changes are allowed under semver. Its docs also note that the Wasm sandbox uses more memory per connection.
- Sources: https://pkg.go.dev/github.com/ncruces/go-sqlite3 and https://github.com/ncruces/go-sqlite3/wiki/Support-matrix

**Verdict:** pick modernc for its v1 stability promise and its plain `database/sql` usage. ncruces is a fine fallback: if modernc ever causes trouble, switching means changing one import and one driver name, because all SQL goes through `database/sql`.

### Opening the database

```go
import (
	"database/sql"
	"net/url"
	"path/filepath"

	_ "modernc.org/sqlite"
)

func Open(path string) (*sql.DB, error) {
	q := url.Values{}
	q.Add("_pragma", "journal_mode(WAL)")
	q.Add("_pragma", "synchronous(NORMAL)")
	q.Add("_pragma", "busy_timeout(5000)")
	q.Add("_pragma", "foreign_keys(1)")
	dsn := "file:" + filepath.ToSlash(path) + "?" + q.Encode() // see Windows notes for drive letters
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(1) // one writer is all SQLite allows anyway; simplest correct choice
	return db, db.Ping()
}
```

Why each setting:

- **Pragmas in the DSN, not in `db.Exec`.** `database/sql` keeps a pool of connections. `foreign_keys` and `busy_timeout` apply to a single connection. If you `Exec` them once, they land on one pooled connection and are missing on the others. The modernc docs say connection state carries over to the next borrower and recommend bounding the pool. SQLite also documents that `foreign_keys` defaults to OFF and must be set per connection.
- **WAL.** Readers do not block the writer, and the writer does not block readers. That suits a UI that reads the curve while the roast loop writes. Only one writer can run at a time. The setting persists in the database file.
- **`synchronous=NORMAL` in WAL mode.** SQLite calls this "the best balance between performance and safety for most applications running in WAL mode". It cannot corrupt the database. A power cut may lose the last committed transaction, but an application crash loses nothing. Losing the last second of a roast after a power cut is acceptable.
- **`SetMaxOpenConns(1)`.** At 1 Hz there is no contention worth tuning. This rules out `SQLITE_BUSY` between Roastro's own connections and keeps the mental model simple. If concurrent reads are needed later, open a second read-only `*sql.DB`.
- Sources: https://www.sqlite.org/pragma.html and https://www.sqlite.org/wal.html

## 2. Migrations

### Options

- **goose** (`github.com/pressly/goose/v3`, v3.29.0, 2026-10-11)
  - Supports embedded migrations: `goose.SetBaseFS(embedFS)` then `goose.Up(db, "migrations")`, or `goose.NewProvider` with an `fs.FS`. It works on a `*sql.DB` and has a SQLite dialect.
  - Adds a dependency tree, a version table, and its own annotation syntax (`-- +goose Up`).
  - Source: https://pkg.go.dev/github.com/pressly/goose/v3
- **golang-migrate** (`github.com/golang-migrate/migrate/v4`, v4.20.1, 2026-09-09)
  - Its `database/sqlite` driver uses modernc. Each migration needs separate `up`/`down` files, and you wire a source driver (`iofs`) to a database driver.
  - That is more moving parts than Roastro needs.
  - Source: https://pkg.go.dev/github.com/golang-migrate/migrate/v4/database/sqlite
- **Homegrown runner on `PRAGMA user_version`**
  - SQLite reserves `user_version` (a 32-bit integer in the file header) for the application. SQLite itself does not use it, and it is the documented place to store a schema version. No bookkeeping table is needed.
  - Source: https://www.sqlite.org/pragma.html#pragma_user_version

### Recommendation: the homegrown runner

Roastro has one database, one process and forward-only migrations. A local desktop app never needs to roll back. Under those conditions the runner is small enough to read in one sitting, and it teaches `embed`, `io/fs` and transactions, which are good material for the study guide.

```
internal/store/migrations/0001_init.sql
internal/store/migrations/0002_add_events.sql
```

```go
//go:embed migrations/*.sql
var migrationsFS embed.FS

// Migrate applies every migration whose number is greater than PRAGMA user_version.
func Migrate(ctx context.Context, db *sql.DB) error {
	var current int
	if err := db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&current); err != nil {
		return fmt.Errorf("read user_version: %w", err)
	}
	names, err := fs.Glob(migrationsFS, "migrations/*.sql") // sorted lexically; zero-pad the numbers
	if err != nil {
		return err
	}
	for i, name := range names {
		version := i + 1 // file N must be NNNN_*.sql; check this in a test
		if version <= current {
			continue
		}
		body, err := migrationsFS.ReadFile(name)
		if err != nil {
			return err
		}
		tx, err := db.BeginTx(ctx, nil)
		if err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, string(body)); err != nil {
			tx.Rollback()
			return fmt.Errorf("migration %s: %w", name, err)
		}
		// PRAGMA does not accept bind parameters; version is an int we control.
		if _, err := tx.ExecContext(ctx, fmt.Sprintf("PRAGMA user_version = %d", version)); err != nil {
			tx.Rollback()
			return err
		}
		if err := tx.Commit(); err != nil {
			return err
		}
	}
	return nil
}
```

Notes:

- **The whole migration is atomic.** SQLite DDL is transactional, and `user_version` is written inside the same transaction as the schema change. A crash mid-migration leaves the old version and the old schema.
- **Multi-statement files work.** modernc runs every statement in the string passed to `Exec`. Cover this in a test that runs all migrations against a temp-dir database.
- **Changing `foreign_keys` mid-migration does nothing.** It "is a no-op inside a transaction". A table-rebuild migration that needs FKs off must toggle them outside the transaction, which this runner does not support. Add that only when a migration actually needs it.
- **Add a test** that the embedded file names are exactly `0001..N` with no gaps.
- **When to switch to goose:** if migrations ever need Go code, down-migrations, or more than one database engine. The `.sql` files carry over almost unchanged.

## 3. Windows gotchas

1. **Paths and DSNs.**
   - Build the path with `filepath.Join`. For a `file:` URI, SQLite's rules are: convert `\` to `/`, add a leading `/` before a drive letter, escape `?` as `%3f` and `#` as `%23`. So `C:\Users\x\AppData\Roaming\Roastro\roastro.db` becomes `file:/C:/Users/x/AppData/Roaming/Roastro/roastro.db?...`.
   - A relative path with a drive letter cannot be converted, so make the path absolute first.
   - modernc also accepts a plain path. It strips the query before opening, but SQLite URI parameters such as `mode=ro` then stop working. Pick one form and write a tiny `dsn(path)` helper with a test for a Windows-style path.
   - Source: https://www.sqlite.org/uri.html
2. **Where to put the file.** Use `os.UserConfigDir()`, which returns `%AppData%` on Windows, `$XDG_CONFIG_HOME` or `~/.config` on Linux, and `~/Library/Application Support` on macOS. Join `Roastro/roastro.db` and run `os.MkdirAll`. Do not write next to the exe: Program Files is not writable. Source: https://pkg.go.dev/os#UserConfigDir
3. **Never on a network share or a synced folder.**
   - WAL "does not work over a network filesystem" because the wal-index is shared memory. On Windows that shared memory is a memory-mapped `-shm` file next to the database.
   - SQLite also lists buggy network-filesystem locking as a corruption cause.
   - A OneDrive-synced folder risks the same problems.
   - Sources: https://www.sqlite.org/wal.html and https://www.sqlite.org/howtocorrupt.html
4. **`-wal` and `-shm` files are part of the database.**
   - Backups and "export" must not copy only `roastro.db` while the app is open. Use `VACUUM INTO 'backup.db'` instead, which works through `db.Exec`.
   - SQLite normally deletes these files when the last connection closes, so call `db.Close()` on shutdown.
   - Source: https://www.sqlite.org/howtocorrupt.html
5. **File locking is mandatory on Windows.**
   - You cannot delete or rename a database another process holds open. SQLite notes that Windows blocks the unlink-while-open pattern that corrupts databases on POSIX.
   - Expect "file in use" errors in tests that `os.Remove` a database before closing it. Use `t.TempDir()` and close the database in `t.Cleanup`.
   - Antivirus or indexers can briefly hold the file. `busy_timeout` covers SQLite's own locks, but not an `os.Rename`.
6. **Only one SQLite library per process.** SQLite warns that two copies of SQLite in one process can silently drop each other's POSIX locks. In Go this means importing only one driver, so do not mix modernc and mattn through some dependency. Source: https://www.sqlite.org/howtocorrupt.html
7. **SQLite version.** sqlite.org documents a rare WAL-reset corruption race in 3.7.0 through 3.51.2, fixed in 3.51.3 and later. modernc v1.61.1 bundles 3.54.1, so stay on a current driver release. Source: https://www.sqlite.org/wal.html

## Sources

- https://proxy.golang.org (`@latest` for each module listed above)
- https://pkg.go.dev/modernc.org/sqlite
- https://pkg.go.dev/github.com/ncruces/go-sqlite3 and https://github.com/ncruces/go-sqlite3/wiki/Support-matrix
- https://github.com/mattn/go-sqlite3
- https://pkg.go.dev/github.com/pressly/goose/v3
- https://pkg.go.dev/github.com/golang-migrate/migrate/v4/database/sqlite
- https://www.sqlite.org/pragma.html
- https://www.sqlite.org/wal.html
- https://www.sqlite.org/uri.html
- https://www.sqlite.org/howtocorrupt.html
- https://pkg.go.dev/os#UserConfigDir
