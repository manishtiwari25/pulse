package board

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ExportStore copies the store to sqlPath (a SQL dump) or dbPath (a SQLite
// file) outside every registered checkout; force overwrites. It returns the
// resolved target path.
func ExportStore(s *store.Store, sqlPath, dbPath string, force bool) (string, error) {
	raw := sqlPath
	if raw == "" {
		raw = dbPath
	}
	if raw == "" {
		return "", refuse(2, "export --sql PATH or --db PATH")
	}
	if strings.HasPrefix(raw, "~") {
		if home, err := os.UserHomeDir(); err == nil {
			raw = home + raw[1:]
		}
	}
	target, err := filepath.Abs(raw)
	if err != nil {
		return "", err
	}
	projects, err := s.Projects(false)
	if err != nil {
		return "", err
	}
	for _, p := range projects {
		if isInside(realPath(target), realPath(p.Path)) {
			return "", refuse(2, "Refusing to export into the registered project %s (%s); pick a path outside every checkout", p.Slug, p.Path)
		}
	}
	if _, err := os.Stat(target); err == nil {
		if !force {
			return "", refuse(2, "%s exists; pass --force to overwrite", target)
		}
		if err := os.Remove(target); err != nil {
			return "", err
		}
	}
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		return "", err
	}
	if sqlPath != "" {
		return target, dumpSQL(s, target)
	}
	_, err = s.DB.Exec("VACUUM INTO ?", target)
	return target, err
}

// dumpSQL writes the schema and every row as SQL statements, the way
// sqlite3's .dump / Python's iterdump does.
func dumpSQL(s *store.Store, target string) error {
	f, err := os.Create(target)
	if err != nil {
		return err
	}
	defer f.Close()
	w := bufio.NewWriter(f)
	defer w.Flush()
	fmt.Fprintln(w, "BEGIN TRANSACTION;")
	rows, err := s.DB.Query("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' " +
		"ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'trigger' THEN 2 ELSE 3 END, name")
	if err != nil {
		return err
	}
	type obj struct{ typ, name, sql string }
	var objs []obj
	for rows.Next() {
		var o obj
		if err := rows.Scan(&o.typ, &o.name, &o.sql); err != nil {
			rows.Close()
			return err
		}
		objs = append(objs, o)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return err
	}
	for _, o := range objs {
		if o.typ != "table" {
			continue
		}
		fmt.Fprintf(w, "%s;\n", strings.TrimSuffix(o.sql, ";"))
		if err := dumpRows(s, w, o.name); err != nil {
			return err
		}
	}
	for _, o := range objs {
		if o.typ != "table" {
			fmt.Fprintf(w, "%s;\n", strings.TrimSuffix(o.sql, ";"))
		}
	}
	fmt.Fprintln(w, "COMMIT;")
	return nil
}

func dumpRows(s *store.Store, w *bufio.Writer, table string) error {
	rows, err := s.DB.Query(fmt.Sprintf(`SELECT * FROM "%s"`, table))
	if err != nil {
		return err
	}
	defer rows.Close()
	cols, err := rows.Columns()
	if err != nil {
		return err
	}
	vals := make([]any, len(cols))
	ptrs := make([]any, len(cols))
	for i := range vals {
		ptrs[i] = &vals[i]
	}
	for rows.Next() {
		if err := rows.Scan(ptrs...); err != nil {
			return err
		}
		lits := make([]string, len(cols))
		for i, v := range vals {
			lits[i] = sqlLiteral(v)
		}
		fmt.Fprintf(w, `INSERT INTO "%s" VALUES(%s);`+"\n", table, strings.Join(lits, ","))
	}
	return rows.Err()
}

func sqlLiteral(v any) string {
	switch x := v.(type) {
	case nil:
		return "NULL"
	case int64:
		return fmt.Sprintf("%d", x)
	case float64:
		return fmt.Sprintf("%g", x)
	case bool:
		if x {
			return "1"
		}
		return "0"
	case []byte:
		return "X'" + fmt.Sprintf("%X", x) + "'"
	case string:
		return "'" + strings.ReplaceAll(x, "'", "''") + "'"
	}
	return "'" + strings.ReplaceAll(fmt.Sprint(v), "'", "''") + "'"
}
