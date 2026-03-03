mod importer;

use rusqlite::{Connection, Row};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{command, Manager};

fn db_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_data_dir().map_err(|e| e.to_string())?;
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
    let key_col = find_column_case_insensitive(&cols, "key").unwrap_or_else(|| "key".to_string());

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
// Stat decoding helpers (Option A / gamer-ish mods)
// -----------------------------

#[derive(Clone, Debug)]
struct StatCostDef {
    descstr: Option<String>, // we'll store descstrpos here (name kept to minimize edits)
    descstrneg: Option<String>,
    #[allow(dead_code)]
    descfunc: Option<i32>,
    #[allow(dead_code)]
    descval: Option<i32>,
}

struct StatDecoder {
    prop_to_stats: HashMap<String, Vec<String>>,
    prop_tooltip: HashMap<String, String>, // NEW
    #[allow(dead_code)]
    stat_cost: HashMap<String, StatCostDef>,
}

impl StatDecoder {
    fn new(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let stat_cost = build_stat_cost(conn).unwrap_or_default();
        let prop_to_stats = build_prop_to_stats(conn).unwrap_or_default();
        let prop_tooltip = build_prop_tooltip(conn).unwrap_or_default(); // NEW
        Ok(Self {
            prop_to_stats,
            prop_tooltip,
            stat_cost,
        })
    }

    /// Convert row prop/par/min/max into gamer-friendly mod strings:
    /// - translates propN -> stat tokens using `properties`
    /// - uses `itemstatcost` desc keys and resolves via `strings` table (key,value)
    /// - formats %+d / %d placeholders
    /// - prints +X or +X-Y ranges
    /// - appends (par) when present (later we’ll decode par into real names)
    fn decode_mods_from_map(
        &self,
        row: &Map<String, Value>,
        strings: &HashMap<String, String>,
    ) -> Vec<String> {
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
                let mut line = self.format_stat_line(&stat, min_i, max_i, strings);

                // Prefer the gamer-facing tooltip from `properties` when present.
                // This fixes oskill / nonclassskill lines (e.g. "+# to [Skill]").
                if let Some(tip) = self.prop_tooltip.get(prop) {
                    if !tip.trim().is_empty() {
                        line = format_from_tooltip(tip, par_v, min_i, max_i);
                    }
                }

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

    fn format_stat_line(
        &self,
        stat: &str,
        min_i: Option<i32>,
        max_i: Option<i32>,
        strings: &HashMap<String, String>,
    ) -> String {
        // Prefer ranges when both min/max present and different
        if let (Some(a), Some(b)) = (min_i, max_i) {
            if a != 0 || b != 0 {
                if a != b {
                    // Use description text but remove placeholders (we're showing explicit range)
                    let desc = self.resolve_desc_text(stat, if b != 0 { b } else { a }, strings);
                    let clean = strip_placeholders(&desc);

                    return if a >= 0 && b >= 0 {
                        format!("+{a}-{b} {clean}")
                    } else {
                        format!("{a}-{b} {clean}")
                    };
                }
                // If same, treat as single value
                return self.format_stat_line(stat, Some(a), None, strings);
            }
        }

        // Single value
        let v = min_i.or(max_i).unwrap_or(0);
        if v == 0 {
            // Just return the desc without placeholders if any
            return strip_placeholders(&self.resolve_desc_text(stat, 0, strings));
        }

        let desc = self.resolve_desc_text(stat, v, strings);

        // If template has placeholders, apply them
        if desc.contains("%d") || desc.contains("%+d") {
            return apply_placeholders(&desc, v);
        }

        // Otherwise fallback to "+v desc"
        if v >= 0 {
            format!("+{v} {desc}")
        } else {
            format!("{v} {desc}")
        }
    }

    fn resolve_desc_text(&self, stat: &str, v: i32, strings: &HashMap<String, String>) -> String {
        let def = self.stat_cost.get(stat);

        let mut key: Option<&String> = None;
        if v < 0 {
            key = def.and_then(|d| d.descstrneg.as_ref());
        }
        if key.is_none() {
            key = def.and_then(|d| d.descstr.as_ref());
        }

        if let Some(k) = key {
            if let Some(s) = strings.get(k) {
                return s.clone();
            }
            // fallback: show the key if it isn't found in strings
            return k.clone();
        }

        // fallback: raw stat token
        stat.to_string()
    }
}
/// Load the entire strings table (key,value) into memory for quick lookups.
fn load_strings_kv(conn: &Connection) -> HashMap<String, String> {
    let mut out = HashMap::new();

    if table_exists(conn, "strings").ok() != Some(true) {
        return out;
    }

    let mut stmt = match conn.prepare("SELECT key, value FROM strings") {
        Ok(s) => s,
        Err(_) => return out,
    };

    let rows = match stmt.query_map([], |r| {
        let k: Option<String> = r.get(0)?;
        let v: Option<String> = r.get(1)?;
        Ok((k.unwrap_or_default(), v.unwrap_or_default()))
    }) {
        Ok(it) => it,
        Err(_) => return out,
    };

    for row in rows.flatten() {
        let (k, v) = row;
        if !k.is_empty() {
            out.insert(k, v);
        }
    }

    out
}

/// Replace D2 placeholders:
///  - %d  -> absolute value
///  - %+d -> signed value
fn format_from_tooltip(tip: &str, par_v: &str, min_i: Option<i32>, max_i: Option<i32>) -> String {
    let mut s = tip.to_string();

    if !par_v.is_empty() {
        s = s.replace("[Skill]", par_v);
    }

    match (min_i, max_i) {
        (Some(a), Some(b)) if a != 0 || b != 0 => {
            if a != b {
                s = s
                    .replace("Min #", "")
                    .replace("Max #", "")
                    .replace("#", &format!("{a}-{b}"));
            } else {
                s = s
                    .replace("Min #", "")
                    .replace("Max #", "")
                    .replace("#", &a.to_string());
            }
        }
        (Some(a), None) if a != 0 => {
            s = s
                .replace("Min #", "")
                .replace("Max #", "")
                .replace("#", &a.to_string());
        }
        (None, Some(b)) if b != 0 => {
            s = s
                .replace("Min #", "")
                .replace("Max #", "")
                .replace("#", &b.to_string());
        }
        _ => {
            s = s.replace("Min #", "").replace("Max #", "").replace("#", "");
        }
    }

    s.trim().to_string()
}

fn apply_placeholders(template: &str, v: i32) -> String {
    let signed = v.to_string();
    let abs = v.abs().to_string();
    template.replace("%+d", &signed).replace("%d", &abs)
}

fn strip_placeholders(s: &str) -> String {
    s.replace("%+d", "").replace("%d", "").trim().to_string()
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

    // OLD (wrong for your file):
    // let descstr_col = cols.iter().find(|c| c.eq_ignore_ascii_case("descstr")).cloned();

    // NEW (correct for your file):
    let descstr_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descstrpos"))
        .cloned();

    let descstrneg_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descstrneg"))
        .cloned();
    let descfunc_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descfunc"))
        .cloned();
    let descval_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("descval"))
        .cloned();

    let get_opt_string =
        |r: &Row<'_>, cols: &[String], col_name: &Option<String>| -> Option<String> {
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

    let sql = format!("SELECT * FROM itemstatcost;");
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |r| {
        let stat: Option<String> = r
            .get(cols.iter().position(|c| c == &stat_col).unwrap_or(0))
            .ok();

        let stat = stat.unwrap_or_default();
        let def = StatCostDef {
            descstr: get_opt_string(r, &cols, &descstr_col),
            descstrneg: get_opt_string(r, &cols, &descstrneg_col),
            descfunc: get_opt_i32(r, &cols, &descfunc_col),
            descval: get_opt_i32(r, &cols, &descval_col),
        };
        Ok((stat, def))
    })?;

    for row in rows {
        let (k, v) = row?;
        if !k.trim().is_empty() {
            stat_cost.insert(k.trim().to_string(), v);
        }
    }

    Ok(stat_cost)
}

fn build_prop_to_stats(conn: &Connection) -> Result<HashMap<String, Vec<String>>, rusqlite::Error> {
    let mut prop_to_stats: HashMap<String, Vec<String>> = HashMap::new();

    if !table_exists(conn, "properties")? {
        return Ok(prop_to_stats);
    }

    let cols = pragma_table_columns(conn, "properties")?;

    let code_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("code"))
        .cloned()
        .unwrap_or_else(|| "code".to_string());

    // stat1..stat7 exist in many mods; we’ll pick all we find.
    let stat_cols: Vec<String> = cols
        .iter()
        .filter(|c| c.to_lowercase().starts_with("stat"))
        .cloned()
        .collect();

    let sql = format!("SELECT * FROM properties;");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let code: Option<String> = r
            .get(cols.iter().position(|c| c == &code_col).unwrap_or(0))
            .ok();

        let mut stats: Vec<String> = Vec::new();
        for sc in &stat_cols {
            if let Some(idx) = cols.iter().position(|c| c == sc) {
                let v: Option<String> = r.get(idx).ok();
                if let Some(v) = v {
                    let t = v.trim();
                    if !t.is_empty() {
                        stats.push(t.to_string());
                    }
                }
            }
        }

        Ok((code.unwrap_or_default(), stats))
    })?;

    for row in rows {
        let (code, stats) = row?;
        let code = code.trim().to_string();
        if code.is_empty() {
            continue;
        }
        let stats = stats
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>();
        if stats.is_empty() {
            continue;
        }
        prop_to_stats.insert(code, stats);
    }

    Ok(prop_to_stats)
}

fn build_prop_tooltip(conn: &Connection) -> Result<HashMap<String, String>, rusqlite::Error> {
    let mut out = HashMap::new();
    if !table_exists(conn, "properties")? {
        return Ok(out);
    }

    let cols = pragma_table_columns(conn, "properties")?;

    let code_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("code"))
        .cloned()
        .unwrap_or_else(|| "code".to_string());

    // Imported header "*Tooltip" becomes "tooltip" after sanitize
    let tooltip_col = cols
        .iter()
        .find(|c| c.eq_ignore_ascii_case("tooltip"))
        .cloned()
        .unwrap_or_else(|| "tooltip".to_string());

    let sql = "SELECT * FROM properties;";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        let code: Option<String> = r
            .get(cols.iter().position(|c| c == &code_col).unwrap_or(0))
            .ok();
        let tip: Option<String> = r
            .get(cols.iter().position(|c| c == &tooltip_col).unwrap_or(0))
            .ok();
        Ok((code.unwrap_or_default(), tip.unwrap_or_default()))
    })?;

    for row in rows {
        let (code, tip) = row?;
        let c = code.trim();
        let t = tip.trim();
        if !c.is_empty() && !t.is_empty() {
            out.insert(c.to_string(), t.to_string());
        }
    }

    Ok(out)
}

#[command]
fn preview_table(
    app: tauri::AppHandle,
    table: String,
    limit: u32,
    rawProps: bool,
) -> Result<Value, String> {
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

    // Load strings once (only needed when we inject mods)
    let strings_kv: HashMap<String, String> = if !rawProps && has_prop_cols {
        load_strings_kv(&conn)
    } else {
        HashMap::new()
    };

    // SELECT * FROM table LIMIT ?
    let sql = format!("SELECT * FROM \"{}\" LIMIT ?1;", table.replace('"', "\"\""));
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
                        .decode_mods_from_map(obj, &strings_kv)
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
// --------------------------
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
