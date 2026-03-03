<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  // ---------- Tauri invoke ----------
  let invokeCmd: ((cmd: string, args?: any) => Promise<any>) | null = null;

  // ---------- UI state ----------
  let status = "Booting…";
  let tables: string[] = [];
  let selected = "";
  let rows: any[] = [];

  let allColumns: string[] = [];
  let visibleColumns: string[] = [];

  let search = "";
  let compact = true;

  // Option B: curated columns + hide prop/min/max families
  let showAllColumns = false; // curated by default
  let showRawProps = false;   // if ON: include prop/min/max columns and show raw text

  let stickyIndex = true;
  let stickyMods = true;

  // ---------- helpers ----------
  function isPropFamily(col: string): boolean {
    // prop1..prop12, par1..par12, min1..min12, max1..max12
    return /^(prop|par|min|max)\d+$/i.test(col);
  }

  function computeVisibleColumns(cols: string[]): string[] {
    // Always keep mods visible if we have it in the list
    if (showAllColumns) {
      // If raw props OFF, still hide prop-family columns
      return cols.filter((c) => (showRawProps ? true : !isPropFamily(c)));
    }

    // Curated set
    const baseCompact = [
      "index",
      "lvl",
      "lvl_req",
      "itemname",
      "mods"
    ];

    const baseWideExtra = [
      "id",
      "version",
      "rarity",
      "disabled",
      "code",
      "spawnable",
      "firstladderseason",
      "lastladderseason",
      "nolimit",
      "chrtransform",
      "invtransform",
      "invfile",
      "carry1",
      "cost_add",
      "cost_mult",
      "flippyfile",
      "dropsound",
      "dropsfxframe",
      "usesound"
    ];

    const keep = new Set<string>(compact ? baseCompact : [...baseCompact, ...baseWideExtra]);

    return cols.filter((c) => {
      if (c === "mods") return true;
      if (!showRawProps && isPropFamily(c)) return false;
      return keep.has(c);
    });
  }

  function formatCell(r: any, c: string): string {
    const v = r?.[c];
    if (v === null || v === undefined || v === "") return "—";
    if (Array.isArray(v)) return v.join(", ");
    return String(v);
  }

  function filteredRows(): any[] {
    const q = search.trim().toLowerCase();
    if (!q) return rows;

    return rows.filter((r) =>
      visibleColumns.some((c) => {
        const v = r?.[c];
        if (v === null || v === undefined) return false;
        if (Array.isArray(v)) return v.join("\n").toLowerCase().includes(q);
        return String(v).toLowerCase().includes(q);
      })
    );
  }

  function recomputeColumnsOnly() {
    visibleColumns = computeVisibleColumns(allColumns);
  }

  // ---------- backend calls ----------
  async function importData() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    try {
      const dir = await open({ directory: true, multiple: false });
      if (!dir) return;

      status = "Importing…";
      await invokeCmd("import_reimagined_data", { baseDir: String(dir) });

      await refreshTables();
      status = "Import complete ✅";
    } catch (e) {
      status = `Import error: ${String(e)}`;
    }
  }

  async function refreshTables() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    try {
      status = "Refreshing tables…";
      tables = await invokeCmd("list_tables");
      if (!selected && tables.length) {
        selected = tables.includes("uniqueitems") ? "uniqueitems" : tables[0];
      }
      await loadPreview();
    } catch (e) {
      status = `Refresh tables error: ${String(e)}`;
      tables = [];
      rows = [];
      allColumns = [];
      visibleColumns = [];
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

      // 1) DB columns
      const cols: string[] = await invokeCmd("table_columns", { table: selected });

      // 2) Preview rows (this is where "mods" is computed)
      rows = await invokeCmd("preview_table", {
        table: selected,
        limit: 50,
        rawProps: showRawProps
      });

      // 3) Build column list:
      const colSet = new Set(cols);
      if (!colSet.has("mods")) {
        if (rows.some((r: any) => r && "mods" in r)) colSet.add("mods");
      }

      allColumns = Array.from(colSet);
      visibleColumns = computeVisibleColumns(allColumns);

      status = `Showing ${rows.length} rows from ${selected}`;
    } catch (e) {
      status = `Load error for ${selected}: ${String(e)}`;
      rows = [];
      allColumns = [];
      visibleColumns = [];
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

  // ✅ Resolve visible keys via lookup_strings
  async function resolveVisibleKeys() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    if (!rows.length) {
      status = "No rows loaded.";
      return;
    }

    try {
      status = "Resolving visible keys…";

      const keys = new Set<string>();
      for (const r of rows) {
        for (const c of visibleColumns) {
          if (c === "mods") continue;
          const v = r?.[c];
          if (typeof v === "string") {
            const s = v.trim();
            if (!s) continue;
            if (s.length > 80) continue;
            if (/^\d+$/.test(s)) continue;
            keys.add(s);
          }
        }
      }

      if (!keys.size) {
        status = "No string keys found in visible cells.";
        return;
      }

      const locale = "enUS";
      const map: Record<string, string> = await invokeCmd("lookup_strings", {
        keys: Array.from(keys),
        locale
      });

      rows = rows.map((r) => {
        const nr = { ...r };
        for (const c of visibleColumns) {
          if (c === "mods") continue;
          const v = nr?.[c];
          if (typeof v === "string") {
            const hit = map[v];
            if (hit && hit.trim().length) nr[c] = hit;
          }
        }
        return nr;
      });

      status = `Resolved ${Object.keys(map).length} keys ✅`;
    } catch (e) {
      status = `Resolve keys error: ${String(e)}`;
    }
  }

  async function debugStringsSchema() {
    if (!invokeCmd) {
      status = "Tauri invoke not found. Are you running inside the app?";
      return;
    }
    try {
      const cols: string[] = await invokeCmd("table_columns", { table: "strings" });
      status = `strings columns: ${cols.join(", ")}`;
    } catch (e) {
      status = `schema error: ${String(e)}`;
    }
  }

  // ---------- init ----------
  onMount(async () => {
    try {
      const core = await import("@tauri-apps/api/core");
      invokeCmd = core.invoke;
      status = "Tauri ready ✅";
      await refreshTables();
    } catch {
      invokeCmd = null;
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

    <button on:click={debugStringsSchema}>Debug Strings Schema</button>

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
      on:click={async () => {
        showRawProps = !showRawProps;
        await loadPreview();
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

    <button on:click={() => (search = "")}>Clear Filter</button>

    <label style="margin-left: 12px;" for="tableSelect">Table:</label>
    <select id="tableSelect" name="tableSelect" bind:value={selected} on:change={loadPreview}>
      {#each tables as t}
        <option value={t}>{t}</option>
      {/each}
    </select>
  </div>

  <p style="margin-top: 10px;"><b>Status:</b> {status}</p>
  <p>Rows loaded: {rows.length} | Rows after filter: {filteredRows().length}</p>

  {#if filteredRows().length}
    <div class="tableWrap">
      <table>
        <thead>
          <tr>
            {#each visibleColumns as c}
              <th
                class:stickyLeftTh={stickyIndex && c === "index"}
                class:stickyRightTh={stickyMods && c === "mods"}
              >
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
    color: white;
    z-index: 2;
  }

  .stickyLeftTh,
  .stickyLeftTd {
    position: sticky;
    left: 0;
    background: white;
    z-index: 3;
  }

  .stickyLeftTh {
    background: #111;
    color: white;
    z-index: 4;
  }

  .stickyRightTh,
  .stickyRightTd {
    position: sticky;
    right: 0;
    background: white;
    z-index: 3;
  }

  .stickyRightTh {
    background: #111;
    color: white;
    z-index: 4;
  }

  .mods ul {
    margin: 0;
    padding-left: 16px;
  }
</style>