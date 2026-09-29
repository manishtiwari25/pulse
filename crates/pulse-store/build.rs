use std::fs;
use std::path::PathBuf;

fn raw_const(src: &str, name: &str) -> String {
    let marker = format!("const {name} = `");
    let start = src.find(&marker).expect("const marker") + marker.len();
    let rest = &src[start..];
    let end = rest.find('`').expect("const end");
    rest[..end].to_string()
}

fn raw_array(src: &str, name: &str) -> Vec<String> {
    let marker = format!("var {name} = []string{{");
    let start = src.find(&marker).expect("array marker") + marker.len();
    let mut items = Vec::new();
    let mut i = start;
    let b = src.as_bytes();
    while i < src.len() {
        while i < src.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= src.len() || b[i] == b'}' {
            break;
        }
        if b[i] == b'/' {
            while i < src.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        assert_eq!(b[i], b'`', "{name}: expected raw string");
        i += 1;
        let item_start = i;
        while b[i] != b'`' {
            i += 1;
        }
        items.push(src[item_start..i].to_string());
        i += 1;
        while i < src.len() && b[i] != b'`' && b[i] != b'}' {
            i += 1;
        }
    }
    items
}

fn quoted_pairs(src: &str, name: &str) -> Vec<(String, String)> {
    let marker = format!("var {name} = map[string]string{{");
    let start = src.find(&marker).expect("map marker") + marker.len();
    let rest = &src[start..src[start..].find("\n}").expect("map end") + start];
    let mut out = Vec::new();
    for line in rest.lines() {
        let line = line.trim();
        if !line.starts_with('"') {
            continue;
        }
        let mut parts = line.split('"');
        parts.next();
        let k = parts.next().unwrap_or_default().to_string();
        parts.next();
        let v = parts.next().unwrap_or_default().to_string();
        out.push((k, v));
    }
    out
}

fn columns(src: &str, name: &str) -> Vec<(String, String, String)> {
    let marker = format!("var {name} = []Column{{");
    let Some(start) = src.find(&marker).map(|i| i + marker.len()) else {
        return Vec::new();
    };
    let rest = &src[start..src[start..].find("\n}").expect("columns end") + start];
    let mut out = Vec::new();
    for line in rest.lines() {
        let line = line.trim();
        if !line.starts_with("{Table:") {
            continue;
        }
        let vals: Vec<_> = line.split('"').skip(1).step_by(2).collect();
        if vals.len() >= 3 {
            out.push((
                vals[0].to_string(),
                vals[1].to_string(),
                vals[2].to_string(),
            ));
        }
    }
    out
}

fn rust_str(s: &str) -> String {
    format!("r####\"{s}\"####")
}

fn emit_array(out: &mut String, name: &str, vals: &[String]) {
    out.push_str(&format!("pub const {name}: &[&str] = &[\n"));
    for v in vals {
        out.push_str("    ");
        out.push_str(&rust_str(v));
        out.push_str(",\n");
    }
    out.push_str("];\n");
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../..")
        .canonicalize()
        .unwrap();
    let store = root.join("internal/store");
    for f in [
        "ddl.go",
        "records_ddl.go",
        "tasks_ddl.go",
        "hooks_ddl.go",
        "runner_ddl.go",
        "evolve_ddl.go",
        "harness_ddl.go",
        "settings.go",
    ] {
        println!("cargo:rerun-if-changed={}", store.join(f).display());
    }

    let mut out = String::new();
    let ddl_src = fs::read_to_string(store.join("ddl.go")).unwrap();
    out.push_str("pub const DDL: &str = ");
    out.push_str(&rust_str(&raw_const(&ddl_src, "DDL")));
    out.push_str(";\n");

    for (file, name, const_name) in [
        ("records_ddl.go", "RecordsDDL", "RECORDS_DDL"),
        ("tasks_ddl.go", "TasksDDL", "TASKS_DDL"),
        ("hooks_ddl.go", "HooksDDL", "HOOKS_DDL"),
        ("runner_ddl.go", "RunnerDDL", "RUNNER_DDL"),
        ("evolve_ddl.go", "EvolveDDL", "EVOLVE_DDL"),
        ("harness_ddl.go", "HarnessDDL", "HARNESS_DDL"),
    ] {
        let src = fs::read_to_string(store.join(file)).unwrap();
        emit_array(&mut out, const_name, &raw_array(&src, name));
        let legacy = name.to_ascii_uppercase();
        out.push_str(&format!("pub const {legacy}: &[&str] = {const_name};\n"));
    }

    let rec_src = fs::read_to_string(store.join("records_ddl.go")).unwrap();
    let rv = rec_src
        .split("const RecordsVersion = \"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap();
    out.push_str(&format!("pub const RECORDS_VERSION: &str = \"{rv}\";\n"));

    let mut cols = Vec::new();
    for (file, name) in [
        ("runner_ddl.go", "RunnerColumns"),
        ("harness_ddl.go", "HarnessColumns"),
    ] {
        let src = fs::read_to_string(store.join(file)).unwrap();
        cols.extend(columns(&src, name));
    }
    out.push_str("pub const ADDITIVE_COLUMNS_DATA: &[(&str, &str, &str)] = &[\n");
    for (t, n, d) in cols {
        out.push_str(&format!(
            "    ({}, {}, {}),\n",
            rust_str(&t),
            rust_str(&n),
            rust_str(&d)
        ));
    }
    out.push_str("];\n");

    let settings_src = fs::read_to_string(store.join("settings.go")).unwrap();
    let mut settings = quoted_pairs(&settings_src, "DefaultSettings");
    settings.sort_by(|a, b| a.0.cmp(&b.0));
    out.push_str("pub const DEFAULT_SETTINGS_DATA: &[(&str, &str)] = &[\n");
    for (k, v) in settings {
        out.push_str(&format!("    ({}, {}),\n", rust_str(&k), rust_str(&v)));
    }
    out.push_str("];\n");

    fs::write(
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ddl_generated.rs"),
        out,
    )
    .unwrap();
}
