use anyhow::{anyhow, Context, Result};
use csv::ReaderBuilder;
use rusqlite::{params_from_iter, Connection};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Window};
use walkdir::WalkDir;

#[derive(Clone, serde::Serialize)]
struct ImportProgress {
    current: usize,
    total: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct ImportSummary {
    pub db_path: PathBuf,
    pub imported: Vec<TableInfo>,
    pub skipped: Vec<String>,
    pub strings_imported: u64,
    pub strings_errors: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct TableInfo {
    pub table: String,
    pub source_file: PathBuf,
    pub rows: u64,
    pub cols: u64,
}

pub fn import_txt_tables_to_sqlite(
    data_dir: &Path,
    db_path: &Path,
    window: Window, // ← window param
) -> Result<ImportSummary> {
    if !data_dir.exists() {
        return Err(anyhow!(
            "Data directory does not exist: {}",
            data_dir.display()
        ));
    }

    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed creating db parent dir: {}", parent.display()))?;
    }

    let mut conn = Connection::open(db_path)
        .with_context(|| format!("Failed opening db: {}", db_path.display()))?;

    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        "#,
    )?;

    let mut best_files: HashMap<String, PathBuf> = HashMap::new();

    for entry in WalkDir::new(data_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let is_txt = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("txt"))
            == Some(true);
        if !is_txt {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }

        let table_key = sanitize_table_name(&stem);

        let new_is_base = path.to_string_lossy().to_lowercase().contains("\\base\\")
            || path.to_string_lossy().to_lowercase().contains("/base/");

        match best_files.get(&table_key) {
            None => {
                best_files.insert(table_key, path);
            }
            Some(existing) => {
                let old_is_base = existing
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("\\base\\")
                    || existing
                        .to_string_lossy()
                        .to_lowercase()
                        .contains("/base/");
                if old_is_base && !new_is_base {
                    best_files.insert(table_key, path);
                }
            }
        }
    }

    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let total = best_files.len();

    let mut processed: usize = 0;
    for (_table_key, path) in best_files.iter() {
        match import_one_txt(&mut conn, path) {
            Ok(table_info) => imported.push(table_info),
            Err(e) => skipped.push(format!("{} :: {}", path.display(), e)),
        }

        processed += 1;

        // === LIVE PROGRESS EMIT ===
        let _ = window.emit(
            "import_progress",
            ImportProgress {
                current: processed,
                total,
            },
        );
    }

    let (strings_imported, strings_errors) = import_strings_json(&mut conn, data_dir)?;

    Ok(ImportSummary {
        db_path: db_path.to_path_buf(),
        imported,
        skipped,
        strings_imported,
        strings_errors,
    })
}

fn import_one_txt(conn: &mut Connection, file_path: &Path) -> Result<TableInfo> {
    let table_name = sanitize_table_name(
        file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("Bad filename"))?,
    );

    let delimiter = detect_delimiter(file_path).unwrap_or(b'\t');

    let mut rdr = ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .flexible(true)
        .from_path(file_path)
        .with_context(|| format!("Failed reading delimited file: {}", file_path.display()))?;

    let headers = rdr.headers()?.clone();
    if headers.is_empty() {
        return Err(anyhow!("No headers found"));
    }

    // If delimiter detection failed and we got 1 giant header, try common fallbacks.
    if headers.len() == 1 {
        let raw = headers.get(0).unwrap_or("");
        if raw.contains('\t') {
            return import_one_txt_with_delim(conn, file_path, b'\t');
        }
        if raw.contains(',') {
            return import_one_txt_with_delim(conn, file_path, b',');
        }
        if raw.contains(';') {
            return import_one_txt_with_delim(conn, file_path, b';');
        }
    }

    import_from_reader(conn, file_path, &table_name, rdr, headers)
}

fn import_one_txt_with_delim(conn: &mut Connection, file_path: &Path, delim: u8) -> Result<TableInfo> {
    let table_name = sanitize_table_name(
        file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("Bad filename"))?,
    );

    let mut rdr = ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(true)
        .flexible(true)
        .from_path(file_path)
        .with_context(|| format!("Failed reading delimited file: {}", file_path.display()))?;

    let headers = rdr.headers()?.clone();
    if headers.is_empty() {
        return Err(anyhow!("No headers found"));
    }

    import_from_reader(conn, file_path, &table_name, rdr, headers)
}

fn import_from_reader(
    conn: &mut Connection,
    file_path: &Path,
    table_name: &str,
    mut rdr: csv::Reader<std::fs::File>,
    headers: csv::StringRecord,
) -> Result<TableInfo> {
    let mut col_names = Vec::with_capacity(headers.len());
    let mut seen = HashSet::new();

    for h in headers.iter() {
        let mut name = sanitize_column_name(h);
        if name.is_empty() {
            name = "col".to_string();
        }

        let mut n = 1;
        let base = name.clone();
        while seen.contains(&name) {
            n += 1;
            name = format!("{}_{}", base, n);
        }

        seen.insert(name.clone());
        col_names.push(name);
    }

    conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", table_name), [])?;

    let cols_sql = col_names
        .iter()
        .map(|c| format!("\"{}\" TEXT", c))
        .collect::<Vec<_>>()
        .join(", ");

    conn.execute(&format!("CREATE TABLE \"{}\" ({});", table_name, cols_sql), [])?;

    let placeholders = (0..col_names.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({});",
        table_name,
        col_names
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders
    );

    let tx = conn.transaction()?;
    let mut stmt = tx.prepare(&insert_sql)?;

    let mut row_count: u64 = 0;
    for result in rdr.records() {
        let record = result?;

        let mut vals: Vec<String> = Vec::with_capacity(col_names.len());
        for i in 0..col_names.len() {
            vals.push(record.get(i).unwrap_or("").to_string());
        }

        stmt.execute(params_from_iter(vals.iter()))?;
        row_count += 1;
    }

    drop(stmt);
    tx.commit()?;

    Ok(TableInfo {
        table: table_name.to_string(),
        source_file: file_path.to_path_buf(),
        rows: row_count,
        cols: col_names.len() as u64,
    })
}

/// ---------- Strings JSON Import ----------

#[derive(Debug, Deserialize)]
struct StringRow {
    #[serde(rename = "Key")]
    key: Option<String>,
    #[serde(rename = "enUS")]
    en_us: Option<String>,
}

/// Imports strings from: data/local/lng/strings/*.json
fn import_strings_json(conn: &mut Connection, data_dir: &Path) -> Result<(u64, Vec<String>)> {
    let strings_dir = data_dir.join("local").join("lng").join("strings");

    if !strings_dir.exists() {
        // Not an error: just means no strings folder chosen
        return Ok((0, vec![]));
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS strings (
          key   TEXT PRIMARY KEY,
          value TEXT
        );
        "#,
    )?;

    let mut imported: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    let tx = conn.transaction()?;
    let mut stmt = tx.prepare("INSERT OR REPLACE INTO strings(key, value) VALUES (?1, ?2);")?;

    for entry in WalkDir::new(&strings_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path();
        if p.extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("json"))
            != Some(true)
        {
            continue;
        }

        let text = match fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("read failed {} :: {}", p.display(), e));
                continue;
            }
        };
        // Strip BOM if present before parsing JSON
        let text = text.trim_start_matches('\u{feff}').to_string();
        let parsed: Result<Vec<StringRow>, _> = serde_json::from_str(&text);
        let rows = match parsed {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("JSON parse failed {} :: {}", p.display(), e));
                continue;
            }
        };

        let mut text = text;

        // Strip UTF-8 BOM if present
        if text.starts_with('\u{feff}') {
            text = text.trim_start_matches('\u{feff}').to_string();
        }

        // Skip empty files safely
        if text.trim().is_empty() {
            errors.push(format!("JSON empty {} :: skipped", p.display()));
            continue;
        }

        for r in rows {
            let Some(k) = r.key.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) else {
                continue;
            };
            let v = r
                .en_us
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            if stmt.execute([k, v.as_str()]).is_ok() {
                imported += 1;
            }
        }
    }

    drop(stmt);
    tx.commit()?;

    Ok((imported, errors))
}

/// Detect delimiter by sampling the first line: choose whichever appears most: \t , ;
fn detect_delimiter(file_path: &Path) -> Result<u8> {
    let mut f = fs::File::open(file_path)?;
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf)?;
    let sample = &buf[..n];

    let mut line = sample;
    if let Some(pos) = sample.iter().position(|b| *b == b'\n') {
        line = &sample[..pos];
    }
    let line = if line.ends_with(b"\r") {
        &line[..line.len() - 1]
    } else {
        line
    };

    let tabs = line.iter().filter(|b| **b == b'\t').count();
    let commas = line.iter().filter(|b| **b == b',').count();
    let semis = line.iter().filter(|b| **b == b';').count();

    let (best, _count) = [(b'\t', tabs), (b',', commas), (b';', semis)]
        .into_iter()
        .max_by_key(|(_d, c)| *c)
        .unwrap();

    Ok(if tabs == 0 && commas == 0 && semis == 0 {
        b'\t'
    } else {
        best
    })
}

fn sanitize_table_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out = format!("t_{}", out);
    }
    if out.is_empty() {
        "table".to_string()
    } else {
        out
    }
}

fn sanitize_column_name(name: &str) -> String {
    let trimmed = name.trim();
    let mut out = String::new();
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '-' || ch == '.' || ch == '*' {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}