<script lang="ts">
  import { project } from "./project.svelte";
  import { baseName } from "./format";
  import SearchBox from "./SearchBox.svelte";
  import { applyTheme, loadTheme, nextTheme, themeLabel } from "./theme";

  let theme = $state(loadTheme());
  function cycleTheme() {
    theme = nextTheme(theme);
    applyTheme(theme);
  }

  let menu = $state<HTMLDetailsElement | null>(null);

  function openRecent(path: string) {
    if (menu) menu.open = false;
    project.open(path);
  }

  const busy = $derived(project.status === "scanning");

  // Live updates: on, switched off, or wanted but the watch could not start.
  const liveState = $derived(!project.live.enabled ? "off" : project.live.watching ? "on" : "problem");
  const liveTitle = $derived(
    liveState === "on"
      ? "Canlı: kaydedilen değişiklikler haritaya hemen yansır. Duraklatmak için tıkla."
      : liveState === "off"
        ? "Duraklatıldı: değişiklikler biriktiriliyor. Sürdürmek için tıkla."
        : `Klasör izlenemiyor: ${project.live.error ?? "bilinmeyen hata"}`,
  );
</script>

<header class="toolbar">
  <div class="group">
    <button class="primary" onclick={() => project.choose()} disabled={busy}>📂 Proje seç</button>
    <details class="menu" bind:this={menu}>
      <summary aria-label="Son açılan projeler" class:disabled={busy || project.recent.length === 0}>▾</summary>
      <ul>
        {#each project.recent as item (item.path)}
          <li>
            <button class="item" onclick={() => openRecent(item.path)} title={item.path}>
              <span class="name">{item.name}</span>
              <span class="meta">{item.buildSystem} · {baseName(item.path)}</span>
            </button>
            <button class="forget" title="Listeden çıkar" aria-label="Listeden çıkar" onclick={() => project.forget(item.path)}
              >✕</button
            >
          </li>
        {:else}
          <li class="empty">Henüz açılmış proje yok</li>
        {/each}
      </ul>
    </details>
  </div>

  {#if project.summary}
    <div class="title">
      <strong>{project.summary.name}</strong>
      <span class="muted">{project.summary.buildSystem} · {project.summary.modules} modül · {project.summary.packages} paket</span>
    </div>
    <SearchBox />
    <div class="group filters" role="group" aria-label="Filtreler">
      <button class="ghost" class:active={project.showSide} onclick={() => (project.showSide = !project.showSide)} aria-pressed={project.showSide} title="Atlas ağacı ve rapor">Panel</button>
      <label><input type="checkbox" checked={project.filters.tests} onchange={(e) => project.setFilters({ tests: e.currentTarget.checked })} /> Test</label>
      <label><input type="checkbox" checked={project.filters.jdk} onchange={(e) => project.setFilters({ jdk: e.currentTarget.checked })} /> JDK</label>
      <label><input type="checkbox" checked={project.filters.external} onchange={(e) => project.setFilters({ external: e.currentTarget.checked })} /> Dış</label>
      <button class="ghost" onclick={() => project.rescan()} disabled={busy} title="Yeniden tara" aria-label="Yeniden tara">⟳</button>
      <button
        class="ghost live"
        class:on={liveState === "on"}
        class:problem={liveState === "problem"}
        onclick={() => project.toggleLive()}
        aria-pressed={project.live.enabled}
        title={liveTitle}
      >
        <span class="dot" aria-hidden="true"></span>{project.live.enabled ? "Canlı" : "Durdu"}
      </button>
    </div>
  {:else}
    <span class="muted">Henüz proje seçilmedi</span>
  {/if}
  <button class="ghost theme" onclick={cycleTheme} title="Tema: {themeLabel[theme]}" aria-label="Tema: {themeLabel[theme]}">◐ {themeLabel[theme]}</button>
</header>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }
  .group { display: flex; align-items: center; gap: 6px; }
  .title { display: flex; align-items: baseline; gap: 10px; min-width: 0; flex: 1; }
  .title strong { white-space: nowrap; }
  .muted { color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .filters label { display: flex; align-items: center; gap: 4px; color: var(--muted); font-size: 13px; cursor: pointer; }
  .filters { margin-left: auto; gap: 12px; }
  .theme { margin-left: auto; white-space: nowrap; }
  .filters + .theme { margin-left: 0; }
  .ghost.active { background: var(--hover); border-color: var(--accent); }
  .live { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
  .live .dot { width: 8px; height: 8px; border-radius: 50%; border: 1.5px solid var(--muted); }
  .live.on .dot { background: var(--ok); border-color: var(--ok); animation: breathe 2.4s ease-in-out infinite; }
  .live.problem .dot { background: var(--warn); border-color: var(--warn); }
  @keyframes breathe { 50% { opacity: 0.35; } }
  @media (prefers-reduced-motion: reduce) { .live.on .dot { animation: none; } }

  .menu { position: relative; }
  .menu summary {
    list-style: none;
    cursor: pointer;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--ink);
  }
  .menu summary::-webkit-details-marker { display: none; }
  .menu summary.disabled { pointer-events: none; opacity: 0.5; }
  .menu ul {
    position: absolute;
    z-index: 20;
    top: calc(100% + 4px);
    left: 0;
    min-width: 280px;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
  }
  .menu li { display: flex; align-items: center; gap: 4px; }
  .menu li.empty { padding: 8px; color: var(--muted); }
  .item {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 6px 8px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--ink);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .item:hover, .item:focus-visible { background: var(--hover); }
  .item .meta { color: var(--muted); font-size: 12px; }
  .forget { background: none; border: none; color: var(--muted); cursor: pointer; padding: 6px; border-radius: 6px; }
  .forget:hover { color: var(--bad); background: var(--hover); }
</style>
