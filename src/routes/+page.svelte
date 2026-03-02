<script lang="ts">
  import { onMount } from "svelte";

  // Tauri invoke + dialog (wired at runtime)
  let invokeCmd: ((cmd: string, args?: any) => Promise<any>) | null = null;
  let openDialog: ((opts: any) => Promise<string | string[] | null>) | null = null;

  // UI state
  let status = "Loading…";
  let lastImport: any = null;

  let tables: string[] = [];
  let selected = "uniqueitems";

  let rows: any[] = [];
  let allColumns: string[] = [];
  let visibleColumns: string[] = [];

  let search = "";
  let compact = true;

  // Raw props toggle (if true, show prop1/par1/min1… columns; if false, hide them)
  let showRawProps = false;

  // Sticky toggles
  let stickyIndex = true;
  let stickyMods = true;

  // Option B: curated columns by default
  let showAllColumns = false;

  const MODS_COL = "mods";

  // Optional: hide a few noisy/system columns too (only when showAllColumns=false)
  const SYSTEM_HIDE = new Set(
    [
      "id",
      "version",
      "enabled",
      "disabled",
      "spawnable",
      "disablechronicle",
      "dropconditionalc",
      "firstladderseason",
      "lastladderseason",
      "nolimit"
    ].map((x) => x.toLowerCase())
  );

  const isRawPropCol = (c: string) =>
    /^prop\d+$/i.test(c) || /^par\d+$/i.test(c) || /^min\d+$/i.test(c) || /^max\d+$/i.test(c);

  function filteredRows(): any[] {
    const q = (search ?? "").toString().trim().toLowerCase();
    if (!q) return rows;

    return rows.filter((r) =>
      visibleColumns.some((c) => {
        const v = r?.[c];

        if (c === MODS_COL) {
          if (Array.isArray(v)) return v.join(" ").toLowerCase().includes(q);
          return (v ?? "").toString().toLowerCase().includes(q);
        }

        return (v ?? "").toString().toLowerCase().includes(q);
      })
    );
  }

  function formatCell(r: any, c: string): string {
    const v = r?.[c];
    if (v === null || v === undefined) return "—";
    if (typeof v === "string" && v.trim() === "") return "—";
    if (Array.isArray(v)) return v.join(", ");
    return String(v);
  }

  // ✅ Option B column picker
  // - Always keeps key columns
  // - Hides prop/par/min/max unless Raw Props is ON
  // - Hides a few system columns unless "All Columns" is ON
  // - Ensures mods exists and is placed at the end
  function pickVisibleColumns(cols: string[]): string[] {
    const lowerMap = new Map(cols.map((c) => [c.toLowerCase(), c]));
    const getCol = (name: string) => lowerMap.get(name.toLowerCase());

    const basePreferred = ["index", "code", "lvl", "rarity", "invfile", "carry1", "cost_add", "cost_mult"];

    // Start with either compact preferred list, or everything
    let out: string[] = [];

    if (compact) {
      for (const k of basePreferred) {
        const hit = getCol(k);
        if (hit) out.push(hit);
      }
    } else {
      out = [...cols];
    }

    // Remove raw prop plumbing if raw props is OFF
    if (!showRawProps) {
      out = out.filter((c) => !isRawPropCol(c));
    }

    // Curated mode: hide system-ish columns
    if (!showAllColumns) {
      out = out.filter((c) => !SYSTEM_HIDE.has(c.toLowerCase()));
    }

    // Ensure uniqueness (preserve order)
    const seen = new Set<string>();
    out = out.filter((c) => {
      const key = c.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });

    // ✅ Always append mods at the end (even if not part of table_columns)
    out = out.filter((c) => c.toLowerCase() !== MODS_COL);
    out.push(MODS_COL);

    // Fallback if compact mode found almost nothing
    if (out.length <= 1) {
      const fallback = cols
        .filter((c) => (showRawProps ? true : !isRawPropCol(c)))
        .filter((c) => (showAllColumns ? true : !SYSTEM_HIDE.has(c.toLowerCase())))
        .slice(0, 12);

      const uniq: string[] = [];
      const s2 = new Set<string>();
      for (const c of fallback) {
        const k = c.toLowerCase();
        if (s2.has(k)) continue;
        s2.add(k);
        uniq.push(c);
      }

      // keep mods last
      return [...uniq.filter((c) => c.toLowerCase() !== MODS_COL), MODS_COL];
    }

    return out;
  }

  function recomputeColumnsOnly() {
    visibleColumns = pickVisibleColumns(allColumns);
  }

  function clearFilter() {
    search = "";
    status = `Showing ${rows.length} rows from ${selected}`;
  }

  async function resolveVisibleKeys() {
    // Placeholder for later (string resolving per visible table)
    status = "Resolve Visible Keys ✅ (UI hook ready — backend wiring later)";
  }

  async function refreshTables() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }

    try {
      tables = await invokeCmd("list_tables");
      if (!tables.includes(selected)) selected = tables[0] ?? "uniqueitems";
      status = `Loaded ${tables.length} tables`;
      await loadPreview();
    } catch (e) {
      status = `Refresh tables error: ${String(e)}`;
    }
  }

  async function loadPreview() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    if (!selected) return;

    try {
      status = `Loading ${selected}…`;

      // fetch columns first
      const cols: string[] = await invokeCmd("table_columns", { table: selected });
      allColumns = cols;
      visibleColumns = pickVisibleColumns(cols);

      // fetch rows
      rows = await invokeCmd("preview_table", {
        table: selected,
        limit: 50,
        rawProps: showRawProps
      });

      status = `Showing ${rows.length} rows from ${selected}`;
    } catch (e) {
      status = `Load error for ${selected}: ${String(e)}`;
      rows = [];
    }
  }

  async function importData() {
    if (!invokeCmd || !openDialog) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }

    try {
      const dir = await openDialog({
        directory: true,
        multiple: false,
        title: "Select the Reimagined.mpq/data folder"
      });

      if (!dir || typeof dir !== "string") {
        status = "Import cancelled.";
        return;
      }

      status = "Importing…";
      lastImport = await invokeCmd("import_reimagined_data", { baseDir: dir });
      status = "Import complete ✅";
      await refreshTables();
    } catch (e) {
      status = `Import error: ${String(e)}`;
    }
  }

  async function countStrings() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    try {
      const n = await invokeCmd("count_strings");
      status = `Strings in DB: ${n}`;
    } catch (e) {
      status = `Count strings error: ${String(e)}`;
    }
  }

  onMount(async () => {
    try {
      // ✅ Use official API (dynamic import so SSR never touches it)
      const core = await import("@tauri-apps/api/core");
      invokeCmd = core.invoke;

      const dialog = await import("@tauri-apps/plugin-dialog");
      openDialog = dialog.open;

      status = "Ready ✅";
      await refreshTables();
    } catch (e) {
      status = "Tauri invoke not found. Are you running inside the app?";
    }
  });
</script>

<main style="padding: 20px;">
  <h1>Reimagined Codex</h1>

  <div class="toolbar">
    <button on:click={importData}>Import Reimagined Data</button>
    <button on:click={refreshTables}>Refresh Tables</button>
    <button on:click={countStrings}>Count Strings</button>
    <button on:click={resolveVisibleKeys}>Resolve Visible Keys</button>

    <span class="pill">View: {compact ? "Compact" : "Wide"}</span>
    <button
      on:click={() => {
        compact = !compact;
        recomputeColumnsOnly();
      }}
    >
      Toggle Compact/Wide
    </button>

    <span class="pill">Columns: {showAllColumns ? "All" : "Curated"}</span>
    <button
      on:click={() => {
        showAllColumns = !showAllColumns;
        recomputeColumnsOnly();
      }}
    >
      Toggle All Columns
    </button>

    <span class="pill">Raw props: {showRawProps ? "On" : "Off"}</span>
    <button
      on:click={() => {
        showRawProps = !showRawProps;
        // raw props affects backend data too, so reload
        loadPreview();
      }}
    >
      Toggle Raw Props
    </button>

    <span class="pill">Index sticky: {stickyIndex ? "On" : "Off"}</span>
    <button on:click={() => (stickyIndex = !stickyIndex)}>Toggle Sticky Index</button>

    <span class="pill">Mods sticky: {stickyMods ? "On" : "Off"}</span>
    <button on:click={() => (stickyMods = !stickyMods)}>Toggle Sticky Mods</button>

    <label style="margin-left: 12px;" for="filterInput">Filter:</label>
    <input id="filterInput" name="filterInput" bind:value={search} placeholder="type to filter…" />
    <button on:click={clearFilter}>Clear Filter</button>

    <label style="margin-left: 12px;" for="tableSelect">Table:</label>
    <select id="tableSelect" name="tableSelect" bind:value={selected} on:change={loadPreview}>
      {#each tables as t}
        <option value={t}>{t}</option>
      {/each}
    </select>
  </div>

  <p style="margin-top: 10px;">Status: {status}</p>
  <p>Rows loaded: {rows.length} | Rows after filter: {filteredRows().length}</p>

  <h2>Last Import</h2>
  {#if lastImport}
    <pre style="max-height: 240px; overflow: auto; border: 1px solid #333; padding: 8px;">
{JSON.stringify(lastImport, null, 2)}
    </pre>
  {:else}
    <p>(No import yet)</p>
  {/if}

  {#if filteredRows().length}
    <div class="tableWrap">
      <table>
        <thead>
          <tr>
            {#each visibleColumns as c}
              <th class:stickyLeftTh={stickyIndex && c === "index"} class:stickyRightTh={stickyMods && c === "mods"}>
                {c}
              </th>
            {/each}
          </tr>
        </thead>

        <tbody>
          {#each filteredRows() as r}
            <tr>
              {#each visibleColumns as c}
                <td
                  class:stickyLeftTd={stickyIndex && c === "index"}
                  class:stickyRightTd={stickyMods && c === "mods"}
                  class:modsCell={c === "mods"}
                >
                  {#if c === "mods" && Array.isArray(r?.mods) && !showRawProps}
                    <div class="mods">
                      <ul>
                        {#each r.mods as line}
                          <li>{line}</li>
                        {/each}
                      </ul>
                    </div>
                  {:else}
                    {formatCell(r, c)}
                  {/if}
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <p>No rows to display. (Try Refresh Tables, pick a table again, or clear the filter.)</p>
  {/if}
</main>

<style>
  .toolbar {
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
  }

  .pill {
    display: inline-block;
    padding: 2px 8px;
    border: 1px solid #333;
    border-radius: 999px;
    font-size: 12px;
    background: #f3f3f3;
  }

  .tableWrap {
    overflow: auto;
    max-height: 70vh;
    border: 1px solid #333;
    padding: 8px;
  }

  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 12px;
  }

  th,
  td {
    padding: 6px;
    border: 1px solid #333;
    white-space: nowrap;
    vertical-align: top;
  }

  th {
    position: sticky;
    top: 0;
    background: #111;
    color: #fff;
    z-index: 5;
  }

  .stickyLeftTh {
    position: sticky;
    left: 0;
    z-index: 7;
    background: #111;
    color: #fff;
  }

  .stickyLeftTd {
    position: sticky;
    left: 0;
    z-index: 6;
    background: #fff;
  }

  .stickyRightTh {
    position: sticky;
    right: 0;
    z-index: 7;
    background: #111;
    color: #fff;
  }

  .stickyRightTd {
    position: sticky;
    right: 0;
    z-index: 6;
    background: #fff;
  }

  .modsCell {
    white-space: normal;
    min-width: 280px;
    max-width: 520px;
  }

  .mods ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .mods li {
    margin-bottom: 2px;
  }
</style>