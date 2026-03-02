mod importer;

use rusqlite::{Connection, Row};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{command, Manager};

fn db_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base.join("reimagined.sqlite"))
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
    let mut stmt = conn.prepare(&format!(
        "PRAGMA table_info(\"{}\");",
        table.replace('"', "\"\"")
    ))?;
    let it = stmt.query_map([], |row| row.get::<_, String>(1))?;
    it.collect::<Result<Vec<_>, _>>()
}

fn find_column_case_insensitive(cols: &[String], wanted: &str) -> Option<String> {
    cols.iter()
        .find(|c| c.eq_ignore_ascii_case(wanted))
        .cloned()
}

// -----------------------------
// Commands
// -----------------------------

#[command]
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

#[command]
fn import_reimagined_data(
    app: tauri::AppHandle,
    base_dir: String,
    window: tauri::Window,
) -> Result<importer::ImportSummary, String> {
    let db = db_path(&app)?;
    let data_dir = PathBuf::from(base_dir);
    importer::import_txt_tables_to_sqlite(&data_dir, &db, window).map_err(|e| e.to_string())
}

#[command]
fn list_tables(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let db = db_path(&app)?;
    if !db.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open(db).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;")
        .map_err(|e| e.to_string())?;

    let it = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;

    let mut out = vec![];
    for x in it {
        out.push(x.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[command]
fn table_columns(app: tauri::AppHandle, table: String) -> Result<Vec<String>, String> {
    let db = db_path(&app)?;
    if !db.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    if !table_exists(&conn, &table).map_err(|e| e.to_string())? {
        return Ok(vec![]);
    }
    pragma_table_columns(&conn, &table).map_err(|e| e.to_string())
}

#[command]
fn count_strings(app: tauri::AppHandle) -> Result<i64, String> {
    let db = db_path(&app)?;
    if !db.exists() {
        return Ok(0);
    }
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    if !table_exists(&conn, "strings").map_err(|e| e.to_string())? {
        return Ok(0);
    }

    // strings schema is usually key/value for your importer
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM strings;", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(n)
}

#[command]
fn lookup_strings(
    app: tauri::AppHandle,
    keys: Vec<String>,
    locale: Option<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    use std::collections::{HashMap, HashSet};

    let db = db_path(&app)?;
    if !db.exists() {
        return Ok(HashMap::new());
    }
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    if !table_exists(&conn, "strings").map_err(|e| e.to_string())? {
        return Ok(HashMap::new());
    }

    let cols = pragma_table_columns(&conn, "strings").map_err(|e| e.to_string())?;

    // key column
    let key_col = find_column_case_insensitive(&cols, "key").unwrap_or_else(|| "key".to_string());

    // value column (your schema is key/value)
    // BUT: keep locale support in case you later import wide locale columns
    let want_locale = locale.unwrap_or_else(|| "enUS".to_string());
    let normalized = want_locale
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();

    let mut candidates = vec![
        want_locale.clone(),
        want_locale.to_lowercase(),
        normalized,
        "value".to_string(),
        "enUS".to_string(),
        "enus".to_string(),
    ];

    let mut val_col: Option<String> = None;
    for cand in candidates.drain(..) {
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
    let mut seen: HashSet<String> = HashSet::new();
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

    let placeholders = (0..uniq.len()).map(|_| "?").collect::<Vec<_>>().join(", ");

    let sql = format!(
        "SELECT \"{k}\", \"{v}\" FROM strings WHERE \"{k}\" IN ({ph});",
        k = key_col.replace('"', "\"\""),
        v = val_col.replace('"', "\"\""),
        ph = placeholders
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;

    let mut out: HashMap<String, String> = HashMap::new();
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
// Stat decoding (Option A pass)
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
    /// properties.code -> [stat1..statN]
    prop_to_stats: HashMap<String, Vec<String>>,
    /// itemstatcost.stat -> desc metadata (used more in later passes)
    #[allow(dead_code)]
    stat_cost: HashMap<String, StatCostDef>,
}

impl StatDecoder {
    fn new(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let stat_cost = build_stat_cost(conn).unwrap_or_default();
        let prop_to_stats = build_prop_to_stats(conn).unwrap_or_default();
        Ok(Self {
            prop_to_stats,
            stat_cost,
        })
    }

    /// Convert row prop/par/min/max into readable-ish mod strings.
    /// - translates propN -> stat tokens using `properties`
    /// - prints +X stat or +X-Y stat
    /// - appends (par) when present (later we’ll decode par into real names)
    fn decode_mods_from_map(&self, row: &Map<String, Value>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();

        let parse_i32 = |s: &str| -> Option<i32> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            t.parse::<i32>().ok()
        };

        for i in 1..=10 {
            let p = format!("prop{}", i);
            let par = format!("par{}", i);
            let min = format!("min{}", i);
            let max = format!("max{}", i);

            let prop = row.get(&p).and_then(|v| v.as_str()).unwrap_or("").trim();
            if prop.is_empty() {
                continue;
            }

            let par_v = row.get(&par).and_then(|v| v.as_str()).unwrap_or("").trim();
            let min_v = row.get(&min).and_then(|v| v.as_str()).unwrap_or("").trim();
            let max_v = row.get(&max).and_then(|v| v.as_str()).unwrap_or("").trim();

            let min_i = parse_i32(min_v);
            let max_i = parse_i32(max_v);

            // prop -> stats, fallback to prop token
            let stats = self
                .prop_to_stats
                .get(prop)
                .cloned()
                .unwrap_or_else(|| vec![prop.to_string()]);

            for stat in stats {
                let mut line = match (min_i, max_i) {
                    (Some(a), Some(b)) if a != 0 || b != 0 => {
                        if a != b {
                            if a >= 0 && b >= 0 {
                                format!("+{a}-{b} {stat}")
                            } else {
                                format!("{a}-{b} {stat}")
                            }
                        } else if a >= 0 {
                            format!("+{a} {stat}")
                        } else {
                            format!("{a} {stat}")
                        }
                    }
                    (Some(a), None) if a != 0 => {
                        if a >= 0 {
                            format!("+{a} {stat}")
                        } else {
                            format!("{a} {stat}")
                        }
                    }
                    (None, Some(b)) if b != 0 => {
                        if b >= 0 {
                            format!("+{b} {stat}")
                        } else {
                            format!("{b} {stat}")
                        }
                    }
                    _ => stat.to_string(),
                };

                if !par_v.is_empty() {
                    line.push_str(&format!(" ({par_v})"));
                }

                out.push(line);
            }
        }

        if out.is_empty() {
            vec!["—".to_string()]
        } else {
            out
        }
    }
}

fn build_stat_cost(conn: &Connection) -> Result<HashMap<String, StatCostDef>, rusqlite::Error> {
    let mut stat_cost: HashMap<String, StatCostDef> = HashMap::new();

    if !table_exists(conn, "itemstatcost")? {
        return Ok(stat_cost);
    }

    let cols = pragma_table_columns(conn, "itemstatcost")?;

    let stat_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("stat"))
        .cloned()
        .unwrap_or_else(|| "stat".to_string());

    let descstr_col = cols.iter().find(|c| c.eq_ignore_ascii_case("descstr")).cloned();
    let descstrneg_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descstrneg"))
        .cloned();
    let descfunc_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descfunc"))
        .cloned();
    let descval_col = cols.iter().find(|c| c.eq_ignore_ascii_case("descval")).cloned();

    let get_opt_string = |r: &Row<'_>, cols: &[String], col_name: &Option<String>| -> Option<String> {
        col_name.as_ref().and_then(|cn| {
            cols.iter()
                .position(|c| c == cn)
                .and_then(|idx| r.get::<_, Option<String>>(idx).ok())
                .flatten()
        })
    };

    let get_opt_i32 = |r: &Row<'_>, cols: &[String], col_name: &Option<String>| -> Option<i32> {
        col_name.as_ref().and_then(|cn| {
            cols.iter()
                .position(|c| c == cn)
                .and_then(|idx| r.get::<_, Option<i32>>(idx).ok())
                .flatten()
        })
    };

    let mut stmt = conn.prepare("SELECT * FROM itemstatcost;")?;
    let mut rows = stmt.query([])?;

    while let Some(r) = rows.next()? {
        let stat_idx = cols.iter().position(|c| *c == stat_col).unwrap_or(0);
        let stat: Option<String> = r.get(stat_idx)?;
        let stat = match stat {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };

        stat_cost.insert(
            stat,
            StatCostDef {
                descstr: get_opt_string(r, &cols, &descstr_col),
                descstrneg: get_opt_string(r, &cols, &descstrneg_col),
                descfunc: get_opt_i32(r, &cols, &descfunc_col),
                descval: get_opt_i32(r, &cols, &descval_col),
            },
        );
    }

    Ok(stat_cost)
}

fn build_prop_to_stats(conn: &Connection) -> Result<HashMap<String, Vec<String>>, rusqlite::Error> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();

    if !table_exists(conn, "properties")? {
        return Ok(out);
    }

    let cols = pragma_table_columns(conn, "properties")?;

    let code_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("code"))
        .cloned()
        .unwrap_or_else(|| "code".to_string());

    let mut stat_cols: Vec<String> = cols
        .iter()
        .filter(|c| {
            let lc = c.to_lowercase();
            lc.starts_with("stat") && lc.chars().skip(4).all(|ch| ch.is_ascii_digit())
        })
        .cloned()
        .collect();

    stat_cols.sort_by_key(|c| c[4..].parse::<usize>().unwrap_or(999));
    if stat_cols.is_empty() {
        return Ok(out);
    }

    let select_cols = std::iter::once(code_col.clone())
        .chain(stat_cols.clone().into_iter())
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!("SELECT {select_cols} FROM properties;");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    while let Some(r) = rows.next()? {
        let code: Option<String> = r.get(0)?;
        let code = match code {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };

        let mut stats: Vec<String> = Vec::new();
        for idx in 1..=stat_cols.len() {
            let s: Option<String> = r.get(idx)?;
            if let Some(s) = s {
                let t = s.trim();
                if !t.is_empty() {
                    stats.push(t.to_string());
                }
            }
        }

        if !stats.is_empty() {
            out.insert(code, stats);
        }
    }

    Ok(out)
}

// -----------------------------
// preview_table (inject mods + hide prop/par/min/max)
// -----------------------------

#[command]
#[allow(non_snake_case)]
fn preview_table(app: tauri::AppHandle, table: String, limit: u32, rawProps: bool) -> Result<Value, String> {
    let db = db_path(&app)?;
    if !db.exists() {
        return Ok(Value::Array(vec![]));
    }
    let conn = Connection::open(db).map_err(|e| e.to_string())?;

    if !table_exists(&conn, &table).map_err(|e| e.to_string())? {
        return Ok(Value::Array(vec![]));
    }

    let cols = pragma_table_columns(&conn, &table).map_err(|e| e.to_string())?;
    if cols.is_empty() {
        return Ok(Value::Array(vec![]));
    }

    let has_prop_cols = cols.iter().any(|c| c.eq_ignore_ascii_case("prop1"));

    // Build decoder once per call (fine for now)
    let decoder = StatDecoder::new(&conn).ok();

    // SELECT * FROM table LIMIT ?
    let sql = format!(
        "SELECT * FROM \"{}\" LIMIT ?1;",
        table.replace('"', "\"\"")
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;

    let rows_iter = stmt
        .query_map([limit as i64], |r| {
            let mut obj = Map::new();

            for (i, c) in cols.iter().enumerate() {
                let v: Option<String> = r.get(i)?;
                match v {
                    Some(s) => {
                        obj.insert(c.clone(), Value::String(s));
                    }
                    None => {
                        obj.insert(c.clone(), Value::String(String::new()));
                    }
                }
            }

            Ok(Value::Object(obj))
        })
        .map_err(|e| e.to_string())?;

    let mut out_rows: Vec<Value> = Vec::new();

    for rv in rows_iter {
        let mut v = rv.map_err(|e| e.to_string())?;

        if !rawProps && has_prop_cols {
            if let Value::Object(ref mut obj) = v {
                if let Some(decoder) = decoder.as_ref() {
                    let mods = decoder
                        .decode_mods_from_map(obj)
                        .into_iter()
                        .map(Value::String)
                        .collect::<Vec<_>>();
                    obj.insert("mods".to_string(), Value::Array(mods));
                } else {
                    obj.insert("mods".to_string(), Value::Array(vec![]));
                }

                // Hide noisy prop/par/min/max from the returned JSON when rawProps=false
                for i in 1..=10 {
                    obj.remove(&format!("prop{}", i));
                    obj.remove(&format!("par{}", i));
                    obj.remove(&format!("min{}", i));
                    obj.remove(&format!("max{}", i));
                }
            }
        }

        out_rows.push(v);
    }

    Ok(Value::Array(out_rows))
}

// -----------------------------
// Tauri run
// -----------------------------

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
            lookup_strings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
