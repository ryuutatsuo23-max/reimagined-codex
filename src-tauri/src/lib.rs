// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod importer;

use rusqlite::{Connection, Row};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tauri::{command, Manager};
// use tauri::Window;     // ← ADD Emitter here

// -----------------------------
// Helpers
// -----------------------------

fn db_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reimagined.sqlite")
        .try_into()
        .map_err(|_| "Failed to build db path".to_string())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1;",
        [table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn pragma_table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, rusqlite::Error> {
    let safe = table.replace('"', "\"\"");
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\");", safe))?;
    let it = stmt.query_map([], |row| row.get::<_, String>(1))?;
    it.collect::<Result<Vec<_>, _>>()
}

fn find_column_case_insensitive(cols: &[String], want: &str) -> Option<String> {
    cols.iter()
        .find(|c| c.eq_ignore_ascii_case(want))
        .cloned()
}

// -----------------------------
// Basic commands
// -----------------------------

#[command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[command]
fn import_reimagined_data(
    app: tauri::AppHandle,
    data_dir: String,
    window: tauri::Window,           // ← NEW: window for progress events
) -> Result<importer::ImportSummary, String> {
    let data_dir = PathBuf::from(data_dir);
    let db_path = db_path(&app)?;

    importer::import_txt_tables_to_sqlite(&data_dir, &db_path, window)
        .map_err(|e| e.to_string())
}

#[command]
fn list_tables(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let db_path = db_path(&app)?;

    // If no DB file yet, just return empty list (UI can say “Import first”)
    if !db_path.exists() {
        return Ok(vec![]);
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

#[command]
fn table_columns(app: tauri::AppHandle, table: String) -> Result<Vec<String>, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    let cols = pragma_table_columns(&conn, &table).map_err(|e| e.to_string())?;
    Ok(cols)
}

#[command]
fn count_strings(app: tauri::AppHandle) -> Result<u64, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(0);
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    // Fix B: if strings table missing, return 0 instead of error
    let exists = table_exists(&conn, "strings").map_err(|e| e.to_string())?;
    if !exists {
        return Ok(0);
    }

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM strings;", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    Ok(n.max(0) as u64)
}

// -----------------------------
// String lookup
// -----------------------------

#[command]
fn lookup_strings(
    app: tauri::AppHandle,
    keys: Vec<String>,
    locale: Option<String>,
) -> Result<HashMap<String, String>, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(HashMap::new());
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    if !table_exists(&conn, "strings").map_err(|e| e.to_string())? {
        return Ok(HashMap::new());
    }

    let cols = pragma_table_columns(&conn, "strings").map_err(|e| e.to_string())?;

    // Find key column
    let key_col = find_column_case_insensitive(&cols, "key").unwrap_or_else(|| "key".to_string());

    // Choose language column robustly:
    // - try enUS/enus
    // - else try requested locale
    // - else try "value"
    let want_locale = locale.unwrap_or_else(|| "value".to_string());
    let mut candidates = vec![
        "enUS".to_string(),
        "enus".to_string(),
        want_locale.clone(),
        want_locale.to_lowercase(),
        "value".to_string(),
    ];

    // Also normalize "en-US" → "enus"
    let normalized = want_locale
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    candidates.push(normalized);

    let mut val_col: Option<String> = None;
    for cand in candidates {
        if let Some(found) = cols.iter().find(|c| c.eq_ignore_ascii_case(&cand)) {
            val_col = Some(found.clone());
            break;
        }
    }
    let val_col = val_col.unwrap_or_else(|| {
        cols.iter()
            .find(|c| !c.eq_ignore_ascii_case(&key_col))
            .cloned()
            .unwrap_or_else(|| "value".to_string())
    });

    // Dedup keys
    let mut uniq: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for k in keys {
        let kk = k.trim().to_string();
        if kk.is_empty() {
            continue;
        }
        if seen.insert(kk.clone()) {
            uniq.push(kk);
        }
    }
    if uniq.is_empty() {
        return Ok(HashMap::new());
    }

    // Build IN (?, ?, ?, ...)
    let placeholders = (0..uniq.len()).map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT \"{k}\", \"{v}\" FROM strings WHERE \"{k}\" IN ({ph});",
        k = key_col.replace('"', "\"\""),
        v = val_col.replace('"', "\"\""),
        ph = placeholders
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;

    let mut out = HashMap::new();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(uniq.iter()), |r| {
            let k: String = r.get(0)?;
            let v: Option<String> = r.get(1)?;
            Ok((k, v.unwrap_or_default()))
        })
        .map_err(|e| e.to_string())?;

    for kv in rows {
        let (k, v) = kv.map_err(|e| e.to_string())?;
        out.insert(k, v);
    }

    Ok(out)
}

// -----------------------------
// Stat decoding (minimal “one line per prop”)
// -----------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct StatCostDef {
    descstr: Option<String>,
    descstrneg: Option<String>,
    descfunc: Option<i32>,
    descval: Option<i32>,
}

struct StatDecoder {
    // property code -> ordered list of stat tokens (future: full decode)
    #[allow(dead_code)]
    prop_to_stats: HashMap<String, Vec<String>>,

    // stat name -> desc metadata (future: full decode)
    #[allow(dead_code)]
    stat_cost: HashMap<String, StatCostDef>,
}

impl StatDecoder {
    fn new(conn: &Connection) -> Result<Self, rusqlite::Error> {
        // Build stat_cost cache (safe against schema variations)
        let mut stat_cost: HashMap<String, StatCostDef> = HashMap::new();

        if table_exists(conn, "itemstatcost")? {
            let cols = pragma_table_columns(conn, "itemstatcost")?;

            let stat_col = find_column_case_insensitive(&cols, "stat").unwrap_or_else(|| "stat".to_string());
            let descstr_col = find_column_case_insensitive(&cols, "descstr");
            let descstrneg_col = find_column_case_insensitive(&cols, "descstrneg");
            let descfunc_col = find_column_case_insensitive(&cols, "descfunc");
            let descval_col = find_column_case_insensitive(&cols, "descval");

            let mut stmt = conn.prepare("SELECT * FROM itemstatcost;")?;
            let mut rows = stmt.query([])?;

            while let Some(r) = rows.next()? {
                let idx_stat = cols.iter().position(|c| c == &stat_col).unwrap_or(0);
                let stat: Option<String> = r.get(idx_stat)?;
                let stat = match stat {
                    Some(s) if !s.trim().is_empty() => s,
                    _ => continue,
                };

                let get_opt_string = |col: &Option<String>, row: &Row<'_>| -> Option<String> {
                    col.as_ref().and_then(|cn| {
                        cols.iter()
                            .position(|c| c == cn)
                            .and_then(|idx| row.get::<_, Option<String>>(idx).ok())
                            .flatten()
                    })
                };
                let get_opt_i32 = |col: &Option<String>, row: &Row<'_>| -> Option<i32> {
                    col.as_ref().and_then(|cn| {
                        cols.iter()
                            .position(|c| c == cn)
                            .and_then(|idx| row.get::<_, Option<i32>>(idx).ok())
                            .flatten()
                    })
                };

                stat_cost.insert(
                    stat,
                    StatCostDef {
                        descstr: get_opt_string(&descstr_col, r),
                        descstrneg: get_opt_string(&descstrneg_col, r),
                        descfunc: get_opt_i32(&descfunc_col, r),
                        descval: get_opt_i32(&descval_col, r),
                    },
                );
            }
        }

        Ok(Self {
            prop_to_stats: HashMap::new(), // (future: fill from properties)
            stat_cost,
        })
    }

    /// Minimal: read prop/par/min/max pattern from the JSON map (from preview_table)
    /// and output one-line-per-prop raw-ish strings (website style).
    fn decode_mods_from_map(&self, row: &serde_json::Map<String, Value>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();

        for i in 1..=10 {
            let p = format!("prop{}", i);
            let par = format!("par{}", i);
            let min = format!("min{}", i);
            let max = format!("max{}", i);

            let prop = row
                .get(&p)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();

            if prop.is_empty() {
                continue;
            }

            let par_v = row.get(&par).and_then(|v| v.as_str()).unwrap_or("").trim();
            let min_v = row.get(&min).and_then(|v| v.as_str()).unwrap_or("").trim();
            let max_v = row.get(&max).and_then(|v| v.as_str()).unwrap_or("").trim();

            let suffix = if par_v.is_empty() {
                String::new()
            } else {
                format!(" ({})", par_v)
            };

            if !min_v.is_empty() && !max_v.is_empty() && min_v != max_v {
                out.push(format!("{prop}: {min_v}-{max_v}{suffix}"));
            } else if !min_v.is_empty() {
                out.push(format!("{prop}: {min_v}{suffix}"));
            } else if !max_v.is_empty() {
                out.push(format!("{prop}: {max_v}{suffix}"));
            } else if !par_v.is_empty() {
                out.push(format!("{prop}: {par_v}"));
            } else {
                out.push(prop.to_string());
            }
        }

        if out.is_empty() {
            vec!["—".to_string()]
        } else {
            out
        }
    }
}

// -----------------------------
// preview_table (adds mods when rawProps=false)
// -----------------------------

#[command]
fn preview_table(
    app: tauri::AppHandle,
    table: String,
    limit: u32,
    raw_props: bool,
) -> Result<serde_json::Value, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(serde_json::Value::Array(vec![]));
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    let safe_table = table.replace('"', "\"\"");
    let cols = pragma_table_columns(&conn, &safe_table).map_err(|e| e.to_string())?;

    let sql = format!("SELECT * FROM \"{}\" LIMIT {};", safe_table, limit);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows_iter = stmt.query([]).map_err(|e| e.to_string())?;

    let decoder = StatDecoder::new(&conn).map_err(|e| e.to_string())?;

    let has_prop_cols = cols.iter().any(|c| c.eq_ignore_ascii_case("prop1"));

    let mut out: Vec<serde_json::Value> = Vec::new();
    while let Some(row) = rows_iter.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();

        for (i, col) in cols.iter().enumerate() {
            let v: Option<String> = row.get(i).map_err(|e| e.to_string())?;
            obj.insert(col.clone(), serde_json::Value::String(v.unwrap_or_default()));
        }

        if !raw_props && has_prop_cols {
            let mods = decoder
                .decode_mods_from_map(&obj)
                .into_iter()
                .map(Value::String)
                .collect::<Vec<_>>();
            obj.insert("mods".to_string(), Value::Array(mods));
        }

        out.push(Value::Object(obj));
    }

    Ok(Value::Array(out))
}

#[tauri::command]
fn debug_strings_schema(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    use rusqlite::Connection;

    let db_path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reimagined.sqlite");

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("PRAGMA table_info(strings);")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?;

    let mut cols = Vec::new();
    for r in rows {
        cols.push(r.map_err(|e| e.to_string())?);
    }

    Ok(cols)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            import_reimagined_data,
            list_tables,
            table_columns,
            preview_table,
            count_strings,
            debug_strings_schema,
            lookup_strings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}