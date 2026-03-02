<script lang="ts">
  import { onMount } from "svelte";

  let status = "Idle";
  let tables: string[] = [];
  let selected = "";
  let rows: any[] = [];
  let allColumns: string[] = [];
  let visibleColumns: string[] = [];

  let search = "";
  let compact = true;
  let showRawProps = false;
  let stickyIndex = true;
  let stickyMods = true;

  let openDialog: any = null;
  let invokeCmd: any = null;

  let lastImport: any = null;
  let showImportDetails = false;
  let showStringErrors = true;

  onMount(async () => {
    status = "Loading Tauri APIs...";
    const dialog = await import("@tauri-apps/plugin-dialog");
    const core = await import("@tauri-apps/api/core");
    openDialog = dialog.open;
    invokeCmd = core.invoke;
    status = "Ready ✅";

    await refreshTables();
  });

  async function refreshTables() {
    if (!invokeCmd) return;
    try {
      const list = await invokeCmd("list_tables");
      tables = Array.isArray(list) ? list : [];
      if (tables.length === 0) {
        status = "No database yet. Import first.";
        selected = "";
        rows = [];
        return;
      }

      status = `Loaded ${tables.length} tables`;
      if (!selected || !tables.includes(selected)) {
        selected = tables.includes("uniqueitems") ? "uniqueitems" : (tables.includes("weapons") ? "weapons" : tables[0]);
      }

      await loadPreview();
    } catch (e) {
      // IMPORTANT: show the real error (don’t mask it as “No database yet”)
      status = `List tables error: ${String(e)}`;
      tables = [];
      selected = "";
      rows = [];
    }
  }

  async function importData() {
    if (!openDialog || !invokeCmd) return;

    const folder = await openDialog({
      directory: true,
      multiple: false,
      title: "Select Reimagined DATA folder (the one that contains global/ and local/)"
    });

    if (!folder || typeof folder !== "string") {
      status = "Cancelled / nothing selected";
      return;
    }

    status = "Importing...";
    try {
      const sum = await invokeCmd("import_reimagined_data", { dataDir: folder });
      lastImport = sum;

      const tablesCount = sum?.imported?.length ?? 0;
      const stringsCount = sum?.strings_imported ?? 0;
      const stringErrors = sum?.strings_errors?.length ?? 0;

      status = `Imported ✅ Tables: ${tablesCount}, Strings: ${stringsCount} (errors: ${stringErrors})`;
      await refreshTables();
    } catch (e) {
      status = "Import error: " + String(e);
    }
  }

  function buildVisibleColumns() {
    // If we have no columns yet, nothing to build
    if (!allColumns || allColumns.length === 0) {
      visibleColumns = [];
      return;
    }

    const hasMods = !showRawProps && allColumns.some((c) => c.toLowerCase() === "prop1");

    if (!compact) {
      // Wide: show everything + mods at end if available
      visibleColumns = [...allColumns];
      if (hasMods && !visibleColumns.includes("mods")) visibleColumns.push("mods");
      return;
    }

    // Compact: show a curated set
    const preferred = [
      "index", "name", "code", "lvl", "lvlreq", "reqlevel", "rarity",
      "invfile", "carry1",
      "cost_add", "cost_mult",
      "prop1", "par1", "min1", "max1",
      "prop2", "par2", "min2", "max2",
      "prop3", "par3", "min3", "max3",
      "prop4", "par4", "min4", "max4",
      "prop5", "par5", "min5", "max5"
    ];

    const lowered = new Map(allColumns.map((c) => [c.toLowerCase(), c]));
    const picked: string[] = [];

    for (const want of preferred) {
      const real = lowered.get(want.toLowerCase());
      if (real) picked.push(real);
    }

    // fallback if preferred list doesn’t match table schema
    if (picked.length === 0) picked.push(...allColumns.slice(0, 12));

    if (hasMods) picked.push("mods");
    visibleColumns = picked;
  }

  async function loadPreview() {
    if (!invokeCmd || !selected) return;

    try {
      status = `Loading columns for ${selected}...`;
      allColumns = await invokeCmd("table_columns", { table: selected });

      buildVisibleColumns();

      status = `Loading ${selected}...`;
      rows = await invokeCmd("preview_table", {
        table: selected,
        limit: 50,
        rawProps: showRawProps
      });

      status = `Showing ${rows.length} rows from ${selected}`;
    } catch (e) {
      rows = [];
      status = `Load error for ${selected}: ${String(e)}`;
    }
  }

  function filteredRows() {
    const q = (search ?? "").toString().trim().toLowerCase();
    if (!q) return rows;

    return rows.filter((r) =>
      visibleColumns.some((c) => {
        if (c === "mods") {
          const m = r?.mods;
          const s = Array.isArray(m) ? m.join(" ") : (m ?? "").toString();
          return s.toLowerCase().includes(q);
        }
        return ((r?.[c] ?? "").toString().toLowerCase().includes(q));
      })
    );
  }

  function clearFilter() {
    search = "";
  }

  async function countStrings() {
    if (!invokeCmd) return;
    try {
      const n = await invokeCmd("count_strings");
      status = `Strings in DB: ${n}`;
    } catch (e) {
      status = `Count strings error: ${String(e)}`;
    }
  }

  // This resolves keys inside the MOD LINES (basic QoL)
  // It uses lookup_strings(keys, locale) and replaces known tokens.
  async function resolveVisibleKeys() {
    if (!invokeCmd) return;

    try {
      // Collect keys from mods lines like:
      // "item_stupidity: 1" or "item_nonclassskill (Hidden Charm Passive): 1"
      const keys = new Set<string>();

      for (const r of rows) {
        const mods = r?.mods;
        if (!Array.isArray(mods)) continue;

        for (const line of mods) {
          const s = (line ?? "").toString();
          const beforeColon = s.split(":")[0].trim();
          if (beforeColon) {
            // token might include "(...)" suffix
            const token = beforeColon.split("(")[0].trim();
            if (token) keys.add(token);

            // inside parentheses might be a string key or already english; still attempt
            const m = beforeColon.match(/\(([^)]+)\)/);
            if (m?.[1]) keys.add(m[1].trim());
          }
        }
      }

      const keyList = Array.from(keys);
      if (keyList.length === 0) {
        status = "No keys found to resolve.";
        return;
      }

      const map = await invokeCmd("lookup_strings", {
        keys: keyList,
        locale: "enUS"
      });

      // Apply replacements
      rows = rows.map((r) => {
        const mods = r?.mods;
        if (!Array.isArray(mods)) return r;

        const newMods = mods.map((line: any) => {
          const s = (line ?? "").toString();

          // Replace the main token (left side)
          const [lhs, ...rest] = s.split(":");
          if (!lhs) return s;
          const rhs = rest.length ? ":" + rest.join(":") : "";

          const lhsTrim = lhs.trim();
          const token = lhsTrim.split("(")[0].trim();
          let replaced = lhsTrim;

          if (map?.[token] && map[token].trim()) {
            replaced = replaced.replace(token, map[token].trim());
          }

          // Replace inside parentheses
          replaced = replaced.replace(/\(([^)]+)\)/g, (full: string, inner: string) => {
            const k = inner.trim();
            if (map?.[k] && map[k].trim()) return `(${map[k].trim()})`;
            return full;
          });

          return replaced + rhs;
        });

        return { ...r, mods: newMods };
      });

      status = "Resolved visible keys ✅";
    } catch (e) {
      status = `Resolve keys error: ${String(e)}`;
    }
  }

  function formatCell(r: any, c: string) {
    const v = r?.[c];

    if (c === "mods") {
      // handled in template (ul)
      return "";
    }

    if (v === null || v === undefined) return "—";
    const s = v.toString();
    return s.trim() === "" ? "—" : s;
  }
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
        buildVisibleColumns();
      }}
    >
      Toggle Compact/Wide
    </button>

    <span class="pill">Raw props: {showRawProps ? "On" : "Off"}</span>
    <button
      on:click={() => {
        showRawProps = !showRawProps;
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
    <select
      id="tableSelect"
      name="tableSelect"
      bind:value={selected}
      on:change={() => loadPreview()}
      disabled={tables.length === 0}
    >
      {#each tables as t}
        <option value={t}>{t}</option>
      {/each}
    </select>
  </div>

  <p>Status: {status}</p>
  <p>Rows loaded: {rows.length}</p>
  <p>Rows after filter: {filteredRows().length}</p>

  <h2>Last Import</h2>
  {#if lastImport}
    <div style="margin-bottom: 8px;">
      <button on:click={() => (showImportDetails = !showImportDetails)}>
        {showImportDetails ? "Hide Import Details" : "Show Import Details"}
      </button>

      <span style="margin-left: 12px;">
        Tables: {lastImport?.imported?.length ?? 0}
        &nbsp;|&nbsp; Strings imported: {lastImport?.strings_imported ?? 0}
        &nbsp;|&nbsp; String errors: {lastImport?.strings_errors?.length ?? 0}
      </span>

      {#if (lastImport?.strings_errors?.length ?? 0) > 0}
        <button style="margin-left: 12px;" on:click={() => (showStringErrors = !showStringErrors)}>
          {showStringErrors ? "Hide String Errors" : "Show String Errors"}
        </button>
      {/if}
    </div>

    {#if showStringErrors && (lastImport?.strings_errors?.length ?? 0) > 0}
      <div style="border: 1px solid #c00; padding: 8px; margin-bottom: 8px; white-space: pre-wrap;">
        {#each lastImport.strings_errors as err}
          {err}
          {"\n"}
        {/each}
      </div>
    {/if}

    {#if showImportDetails}
      <pre style="border:1px solid #333; padding:8px; max-height: 200px; overflow:auto;">
{JSON.stringify(lastImport, null, 2)}
      </pre>
    {/if}
  {:else}
    <p>(No import yet)</p>
  {/if}

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
    gap: 10px;
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

  th, td {
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

  .mods {
    white-space: normal;
    min-width: 280px;
    max-width: 520px;
    font-size: 12px;
    line-height: 1.35;
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