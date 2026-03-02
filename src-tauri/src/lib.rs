// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod importer;

use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{command, Manager};
// use tauri::Window;     // ← (unused right now)

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

// Build prop_to_stats from the "properties" table.
// properties.code -> [properties.stat1, properties.stat2, ...]
#[allow(dead_code)]
fn build_prop_to_stats(conn: &Connection) -> Result<HashMap<String, Vec<String>>, rusqlite::Error> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();

    if !table_exists(conn, "properties")? {
        return Ok(out);
    }

    let cols = pragma_table_columns(conn, "properties")?;

    let code_col =
        find_column_case_insensitive(&cols, "code").unwrap_or_else(|| "code".to_string());

    // Find statN columns
    let mut stat_cols: Vec<String> = cols
        .iter()
        .filter(|c| {
            let lc = c.to_lowercase();
            lc.starts_with("stat")
                && lc.len() > 4
                && lc[4..].chars().all(|ch| ch.is_ascii_digit())
        })
        .cloned()
        .collect();

    // Sort stat1..statN
    stat_cols.sort_by_key(|c| {
        c.to_lowercase()
            .trim_start_matches("stat")
            .parse::<u32>()
            .unwrap_or(9999)
    });

    if stat_cols.is_empty() {
        return Ok(out);
    }

    let select_cols = std::iter::once(code_col.clone())
        .chain(stat_cols.clone())
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!("SELECT {} FROM properties;", select_cols);

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    while let Some(r) = rows.next()? {
        let code: Option<String> = r.get(0)?;
        let code = match code {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };

        let mut stats: Vec<String> = Vec::new();
        for i in 0..stat_cols.len() {
            let v: Option<String> = r.get(1 + i)?;
            if let Some(s) = v {
                let s = s.trim();
                if !s.is_empty() {
                    stats.push(s.to_string());
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
// Import commands
// -----------------------------

#[command]
fn import_reimagined_data(
    app: tauri::AppHandle,
    window: tauri::Window,
    data_dir: String,
) -> Result<importer::ImportSummary, String> {
    let data_dir = PathBuf::from(data_dir);

    let db_path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reimagined.sqlite");

    importer::import_txt_tables_to_sqlite(&data_dir, &db_path, window).map_err(|e| e.to_string())
}

#[command]
fn list_tables(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let db_path = db_path(&app)?;
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
    let safe_table = table.replace('"', "\"\"");
    pragma_table_columns(&conn, &safe_table).map_err(|e| e.to_string())
}

#[tauri::command]
fn debug_table_schema(
    app: tauri::AppHandle,
    table: String,
) -> Result<Vec<String>, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(vec![]);
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    if !table_exists(&conn, &table).map_err(|e| e.to_string())? {
        return Ok(vec![]);
    }

    pragma_table_columns(&conn, &table).map_err(|e| e.to_string())
}

// -----------------------------
// Strings
// -----------------------------

#[command]
fn count_strings(app: tauri::AppHandle) -> Result<u64, String> {
    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(0);
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    if !table_exists(&conn, "strings").map_err(|e| e.to_string())? {
        return Ok(0);
    }

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM strings;", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    Ok(n.max(0) as u64)
}

#[command]
fn lookup_strings(
    app: tauri::AppHandle,
    keys: Vec<String>,
    locale: Option<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    use rusqlite::Connection;
    use std::collections::{HashMap, HashSet};

    let db_path = db_path(&app)?;
    if !db_path.exists() {
        return Ok(HashMap::new());
    }

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

    if !table_exists(&conn, "strings").map_err(|e| e.to_string())? {
        return Ok(HashMap::new());
    }

    let cols = pragma_table_columns(&conn, "strings").map_err(|e| e.to_string())?;

    // Key column (usually "key")
    let key_col = find_column_case_insensitive(&cols, "key").unwrap_or_else(|| "key".to_string());

    // Choose value column robustly. Your schema is key/value, so "value" is correct.
    let want_locale = locale.unwrap_or_else(|| "enUS".to_string());
    let normalized = want_locale
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();

    let mut candidates = vec![
        "enUS".to_string(),
        "enus".to_string(),
        want_locale.clone(),
        want_locale.to_lowercase(),
        normalized,
        "value".to_string(),
    ];

    let mut val_col: Option<String> = None;
    for cand in candidates.drain(..) {
        if let Some(found) = cols.iter().find(|c| c.eq_ignore_ascii_case(&cand)) {
            val_col = Some(found.clone());
            break;
        }
    }

    let val_col = val_col.unwrap_or_else(|| {
        // Fallback order:
        // 1) enUS if present
        // 2) value if present
        // 3) first non-(id/key) column
        // 4) value (last resort)
        find_column_case_insensitive(&cols, "enUS")
            .or_else(|| find_column_case_insensitive(&cols, "value"))
            .or_else(|| {
                cols.iter()
                    .find(|c| !c.eq_ignore_ascii_case("id") && !c.eq_ignore_ascii_case(&key_col))
                    .cloned()
            })
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

    // Build IN (?, ?, ...)
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
// Stat decoding (WIP)
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
    // property code -> ordered list of stat tokens (from properties)
    #[allow(dead_code)]
    prop_to_stats: HashMap<String, Vec<String>>,

    // stat name -> desc metadata (from itemstatcost)
    #[allow(dead_code)]
    stat_cost: HashMap<String, StatCostDef>,

    // string key -> localized text (from strings table; schema is key/value in your DB)
    strings: HashMap<String, String>,
}


fn load_properties(conn: &Connection) -> Result<HashMap<String, Vec<String>>, rusqlite::Error> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    if !table_exists(conn, "properties")? {
        return Ok(out);
    }

    let cols = pragma_table_columns(conn, "properties")?;
    let code_col = find_column_case_insensitive(&cols, "code").unwrap_or_else(|| "code".to_string());

    // Collect statN columns in numeric order
    let mut stat_cols: Vec<(usize, String)> = Vec::new();
    for c in &cols {
        let lc = c.to_lowercase();
        if lc.starts_with("stat") {
            // stat1, stat2, ...
            let n = lc[4..].parse::<usize>().unwrap_or(0);
            stat_cols.push((n, c.clone()));
        }
    }
    stat_cols.sort_by_key(|(n, _)| *n);

    let mut stmt = conn.prepare("SELECT * FROM properties;")?;
    let mut rows = stmt.query([])?;

    while let Some(r) = rows.next()? {
        let idx_code = cols.iter().position(|c| c == &code_col).unwrap_or(0);
        let code: Option<String> = r.get(idx_code)?;
        let code = match code {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };

        let mut stats: Vec<String> = Vec::new();
        for (_n, cn) in &stat_cols {
            if let Some(idx) = cols.iter().position(|c| c == cn) {
                let v: Option<String> = r.get(idx).ok();
                if let Some(v) = v {
                    let t = v.trim();
                    if !t.is_empty() {
                        stats.push(t.to_string());
                    }
                }
            }
        }
        out.insert(code, stats);
    }

    Ok(out)
}

fn load_itemstatcost(conn: &Connection) -> Result<HashMap<String, StatCostDef>, rusqlite::Error> {
    let mut stat_cost: HashMap<String, StatCostDef> = HashMap::new();
    if !table_exists(conn, "itemstatcost")? {
        return Ok(stat_cost);
    }

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
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => continue,
        };

        let get_opt_string = |col: &Option<String>| -> Option<String> {
            col.as_ref().and_then(|cn| {
                cols.iter()
                    .position(|c| c == cn)
                    .and_then(|idx| r.get::<_, Option<String>>(idx).ok())
                    .flatten()
            })
        };
        let get_opt_i32 = |col: &Option<String>| -> Option<i32> {
            col.as_ref().and_then(|cn| {
                cols.iter()
                    .position(|c| c == cn)
                    .and_then(|idx| r.get::<_, Option<i32>>(idx).ok())
                    .flatten()
            })
        };

        stat_cost.insert(
            stat,
            StatCostDef {
                descstr: get_opt_string(&descstr_col),
                descstrneg: get_opt_string(&descstrneg_col),
                descfunc: get_opt_i32(&descfunc_col),
                descval: get_opt_i32(&descval_col),
            },
        );
    }

    Ok(stat_cost)
}

fn load_strings(conn: &Connection) -> Result<HashMap<String, String>, rusqlite::Error> {
    let mut out: HashMap<String, String> = HashMap::new();
    if !table_exists(conn, "strings")? {
        return Ok(out);
    }

    let cols = pragma_table_columns(conn, "strings")?;
    // Your schema is `key, value`
    let key_col = find_column_case_insensitive(&cols, "key").unwrap_or_else(|| "key".to_string());
    let val_col = find_column_case_insensitive(&cols, "value").unwrap_or_else(|| "value".to_string());

    let sql = format!(
        "SELECT \"{}\", \"{}\" FROM strings;",
        key_col.replace('"', "\"\""),
        val_col.replace('"', "\"\"")
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let k: String = r.get(0)?;
        let v: Option<String> = r.get(1)?;
        Ok((k, v.unwrap_or_default()))
    })?;

    for kv in rows {
        let (k, v) = kv?;
        if !k.trim().is_empty() && !v.trim().is_empty() {
            out.insert(k, v);
        }
    }

    Ok(out)
}

impl StatDecoder {
    fn new(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let prop_to_stats = load_properties(conn).unwrap_or_default();
        let stat_cost = load_itemstatcost(conn).unwrap_or_default();
        let strings = load_strings(conn).unwrap_or_default();

        Ok(Self {
            prop_to_stats,
            stat_cost,
            strings,
        })
    }


    /// Decode mods from a DB row (serde_json object) into pretty formatted strings.
        /// Compat wrapper (older UI calls this name)
    fn decode_mods_from_map(&self, row: &serde_json::Map<String, Value>) -> Vec<String> {
        self.decode_mods_from_row(row)
    }

fn decode_mods_from_row(&self, row: &serde_json::Map<String, Value>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();

        for i in 1..=10 {
            let p = format!("prop{}", i);
            let par = format!("par{}", i);
            let min = format!("min{}", i);
            let max = format!("max{}", i);

            let prop = row.get(&p).and_then(|v| v.as_str()).unwrap_or("").trim();
            if prop.is_empty() || prop == "—" {
                continue;
            }

            // Values come from SQLite as strings (because importer stores TEXT).
            // So we parse i32 from either JSON string or number safely.
            let parse_i32 = |v: Option<&Value>| -> i32 {
                match v {
                    Some(Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
                    Some(Value::String(s)) => s.trim().parse::<i32>().unwrap_or(0),
                    _ => 0,
                }
            };

            let par_i = parse_i32(row.get(&par));
            let min_i = parse_i32(row.get(&min));
            let max_v = row.get(&max);
            let max_i = {
                let empty = match max_v {
                    None => true,
                    Some(Value::String(s)) => s.trim().is_empty(),
                    _ => false,
                };
                if empty { min_i } else { parse_i32(max_v) }
            };

            out.extend(self.decode_one_prop(prop, par_i, min_i, max_i));
        }

        if out.is_empty() {
            vec!["—".to_string()]
        } else {
            out
        }
    }

    /// Decode a single property code like "str" into 1..N formatted stat lines.
    fn decode_one_prop(&self, prop: &str, par: i32, min: i32, max: i32) -> Vec<String> {
        // Best-effort "true-ish" decoding:
        // - properties.txt maps prop code -> one or more stat tokens
        // - itemstatcost.txt tells us how to format those stat tokens (DescFunc/DescStr…)
        // - strings table resolves DescStr keys to readable text
        //
        // For now we keep it simple and feed (min/max) into the formatter.
        // If min is 0 but par is non-zero (common for “skill id”, etc), we feed par as the value.
        let a = if min != 0 { min } else { par };
        let b = if max != 0 && max != a { Some(max) } else { None };

        if let Some(stats) = self.prop_to_stats.get(prop) {
            if stats.is_empty() {
                return vec![prop.to_string()];
            }

            // Option A: one line per prop (website style) — join multiple stats into one line.
            let parts: Vec<String> = stats
                .iter()
                .filter(|s| !s.trim().is_empty())
                .map(|stat| self.format_stat(stat, a, b))
                .collect();

            if parts.is_empty() {
                vec![prop.to_string()]
            } else {
                vec![parts.join(", ")]
            }
        } else {
            // Fallback if we don't know this prop: show raw values
            let suffix = if par != 0 { format!(" ({})", par) } else { String::new() };
            if min != 0 && max != 0 && min != max {
                vec![format!("{prop}: {min}-{max}{suffix}")]
            } else if min != 0 {
                vec![format!("{prop}: {min}{suffix}")]
            } else if max != 0 {
                vec![format!("{prop}: {max}{suffix}")]
            } else if par != 0 {
                vec![format!("{prop}: {par}")]
            } else {
                vec![prop.to_string()]
            }
        }
    }


    /// High-level formatting, uses itemstatcost when possible.
    fn format_stat(&self, stat: &str, a: i32, b: Option<i32>) -> String {
        // If we have metadata for this stat, use it.
        if let Some(sc) = self.stat_cost.get(stat) {
            let descfunc = sc.descfunc.unwrap_or(0);
            let descstr = sc.descstr.as_deref().unwrap_or(stat);

            // Use strings lookup if descstr is a string key in strings table
            let label = self.str_lookup(descstr);

            let x = a;
            let y = b.unwrap_or(a);

            // Keep it simple & readable for now (Option A)
            // We'll evolve this later into "true stat formatting engine".
            return match descfunc {
                1 => fmt_plus_value(&label, x, y, ""),      // +X Label
                2 => fmt_plus_value(&label, x, y, "%"),     // +X% Label
                3 => fmt_plain_value(&label, x, y, ""),     // Label: X
                4 => fmt_plain_value(&label, x, y, "%"),    // Label: X%
                5 => fmt_prefix_value(&label, x, y, "%"),   // X% Label
                6 => fmt_prefix_value(&label, x, y, ""),    // X Label
                _ => fmt_plus_value(&label, x, y, ""),      // default
            }
        }

        // Fallback (no itemstatcost entry): still make it pretty-ish
        let label = self.str_lookup(stat);
        fmt_plus_value(&label, a, b.unwrap_or(a), "")
    }

    /// Format one stat entry. Handles "dmg%" special and small cases.
    #[allow(dead_code)]
    fn format_one_stat(&self, stat: &str, _par: &str, min: i32, max: i32) -> String {
        // For now we ignore `par` (it matters for skills, charges, etc — later)
        // Keep output clean and consistent.
        let b = if max != min { Some(max) } else { None };
        self.format_stat(stat, min, b)
    }

    fn str_lookup(&self, key_or_text: &str) -> String {
        // strings table schema: key, value
        if let Some(v) = self.strings.get(key_or_text) {
            if !v.trim().is_empty() {
                return v.clone();
            }
        }

        // fallback: make raw keys nicer
        // "item_openwounds" -> "Item Openwounds" (we'll improve later)
        key_or_text
            .replace('_', " ")
            .trim()
            .to_string()
    }
}

/// helpers
fn fmt_plus_value(label: &str, a: i32, b: i32, suffix: &str) -> String {
    if a == b {
        format!("+{}{} {}", a, suffix, label).trim().to_string()
    } else {
        format!("+{}-{}{} {}", a, b, suffix, label).trim().to_string()
    }
}

fn fmt_plain_value(label: &str, a: i32, b: i32, suffix: &str) -> String {
    if a == b {
        format!("{}: {}{}", label, a, suffix).trim().to_string()
    } else {
        format!("{}: {}-{}{}", label, a, b, suffix).trim().to_string()
    }
}

fn fmt_prefix_value(label: &str, a: i32, b: i32, suffix: &str) -> String {
    if a == b {
        format!("{}{} {}", a, suffix, label).trim().to_string()
    } else {
        format!("{}-{}{} {}", a, b, suffix, label).trim().to_string()
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
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

    // decoder caches for this call (ok if it fails)
    let decoder = StatDecoder::new(&conn).ok();

    let mut out: Vec<serde_json::Value> = Vec::new();

    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();
        for (i, col) in cols.iter().enumerate() {
            let v: Option<String> = row.get(i).unwrap_or(None);
            obj.insert(col.clone(), serde_json::Value::String(v.unwrap_or_default()));
        }

        // Inject mods if we have prop fields and raw_props == false
        if !raw_props {
            if let Some(decoder) = decoder.as_ref() {
                let mods = decoder.decode_mods_from_map(&obj);
                obj.insert(
                    "mods".to_string(),
                    serde_json::Value::Array(mods.into_iter().map(Value::String).collect()),
                );
            }
        }

        out.push(serde_json::Value::Object(obj));
    }

    Ok(serde_json::Value::Array(out))
}

// -----------------------------
// Basics / run
// -----------------------------

#[command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
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
            lookup_strings,
            debug_table_schema,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}