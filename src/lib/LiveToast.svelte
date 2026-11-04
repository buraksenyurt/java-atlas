<script lang="ts">
  // A short note after a live update: what changed, how long it took.
  // It goes away by itself; failures stay a little longer.
  import { baseName, duration } from "./format";
  import { project } from "./project.svelte";

  let visible = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    const n = project.notice;
    if (!n) return;
    visible = true;
    clearTimeout(timer);
    timer = setTimeout(() => (visible = false), n.kind === "failed" ? 9000 : 4000);
  });

  const MAX_NAMES = 3;
</script>

{#if visible && project.notice}
  {@const n = project.notice}
  <div class="toast" class:failed={n.kind === "failed"} role="status" aria-live="polite">
    {#if n.kind === "failed"}
      <strong>Değişiklik okunamadı</strong>
      <span class="detail">{n.message}</span>
      <span class="muted">Harita bir önceki hâliyle duruyor; dosyayı düzeltip kaydedince güncellenir.</span>
    {:else if n.full}
      <strong>⟳ {n.trigger ? `${baseName(n.trigger)} değişti` : "Klasör değişti"} — proje yeniden tarandı</strong>
      <span class="muted">{duration(n.ms)}</span>
    {:else}
      <strong>⟳ {n.fileCount} dosya güncellendi</strong>
      <span class="detail">
        {n.files.slice(0, MAX_NAMES).map(baseName).join(", ")}{n.fileCount > MAX_NAMES ? ` +${n.fileCount - MAX_NAMES}` : ""}
      </span>
      <span class="muted">{duration(n.ms)}</span>
    {/if}
    <button class="close" onclick={() => (visible = false)} aria-label="Kapat">✕</button>
  </div>
{/if}

<style>
  .toast {
    position: absolute;
    /* Bottom left of the map: the legend owns the bottom right. */
    left: 14px;
    bottom: 14px;
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 10px;
    max-width: min(520px, 60%);
    padding: 8px 34px 8px 12px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-left: 4px solid var(--ok);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.16);
    font-size: 13px;
    z-index: 25;
  }
  .toast.failed { border-left-color: var(--bad); }
  .detail { font-family: ui-monospace, "Cascadia Code", "JetBrains Mono", Menlo, monospace; font-size: 12px; overflow-wrap: anywhere; }
  .muted { color: var(--muted); font-size: 12px; }
  .close { position: absolute; right: 6px; top: 6px; background: none; border: none; color: var(--muted); cursor: pointer; padding: 2px 6px; }
</style>
