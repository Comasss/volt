import "@fontsource/jetbrains-mono/latin-300.css";
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/jetbrains-mono/latin-500.css";
import "@fontsource/inter/latin-400.css";
import "@fontsource/inter/latin-500.css";
import "./styles.css";

import { createBackend, isTauri, type Backend } from "./backend";
import { setLang, t, type Key } from "./i18n";
import { icon, type IconName } from "./icons";
import type {
  AppState,
  Connection,
  Control,
  ControlSection,
  ControlValue,
  Device,
  DeviceControls,
  DeviceKind,
  HidRow,
  Settings,
} from "./types";

type View = "devices" | "device" | "diag" | "settings";

const ui = {
  view: "devices" as View,
  selected: null as string | null,
  hid: null as HidRow[] | null,
  hidError: null as string | null,
  hidLoading: false,
  /** Huella del dispositivo cuyo nombre se está editando. */
  editing: null as string | null,
  /** Estado llegado mientras se edita: se aplica al terminar para no perder el foco. */
  pending: null as AppState | null,
  /** Dispositivo abierto en la vista «Personalizar». */
  detail: null as string | null,
  controls: null as DeviceControls | null,
  ctlLoading: false,
  ctlError: null as string | null,
  /** Control que se está escribiendo ahora (bloquea el resto). */
  ctlBusy: null as string | null,
};

let state: AppState;
let backend: Backend;

const $ = (id: string) => document.getElementById(id)!;

// ---------------------------------------------------------------------------
// Formato
// ---------------------------------------------------------------------------

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

const pad = (n: number, w = 2) => String(n).padStart(w, "0");
const hex = (n: number) => n.toString(16).toUpperCase().padStart(4, "0");

function clock(ms: number, seconds = true): string {
  const d = new Date(ms);
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}${seconds ? ":" + pad(d.getSeconds()) : ""}`;
  if (d.toDateString() === new Date().toDateString()) return time;
  return `${pad(d.getDate())} ${t("months").split(",")[d.getMonth()]} ${time}`;
}

function interval(secs: number): string {
  return secs < 60 ? `${secs} S` : `${secs / 60} MIN`;
}

const KIND_ICON: Record<DeviceKind, IconName> = {
  mouse: "mouse",
  keyboard: "keyboard",
  headset: "headset",
  gamepad: "gamepad",
  other: "other",
};

const LINK_ICON: Record<Connection, IconName> = {
  receiver: "receiver",
  wired: "wired",
  bluetooth: "bluetooth",
  virtual: "virtual",
};

const GROUP_ORDER: Connection[] = ["receiver", "wired", "bluetooth", "virtual"];

/** Los dispositivos demo se agrupan en "DEMO" aunque simulen otra conexión. */
const groupOf = (d: Device): Connection => (d.provider === "mock" ? "virtual" : d.connection);

const percentOf = (d: Device): number | null => d.battery?.percent ?? null;

// ---------------------------------------------------------------------------
// Tema e idioma
// ---------------------------------------------------------------------------

const darkQuery = matchMedia("(prefers-color-scheme: dark)");

function applyTheme() {
  const th = state.settings.theme;
  const dark = th === "dark" || (th === "system" && darkQuery.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

function applyLang() {
  setLang(state.lang);
  document.querySelector(".brand-sub")!.textContent = t("monitor");
  $("btn-scan").innerHTML = `${icon("scan")}<span>${t("scan")}</span>`;
  $("btn-scan").title = t("scanTitle");
  $("btn-settings").innerHTML = `${icon("sliders")}<span>${t("settings")}</span>`;
  $("btn-min").title = t("minimize");
  $("btn-close").title = t("closeTitle");
}

// ---------------------------------------------------------------------------
// Árbol
// ---------------------------------------------------------------------------

function treeLeaf(d: Device): string {
  const p = percentOf(d);
  const st = d.status.state;
  const val = st === "unavailable" ? t("na") : st === "offline" ? "OFF" : p == null ? "—" : String(p);
  const cls = [
    "tree-row",
    "leaf",
    d.low ? "is-low" : "",
    st !== "online" ? "is-dim" : "",
    ui.selected === d.id ? "is-selected" : "",
  ].join(" ");
  return `<li><button class="${cls}" data-device="${esc(d.id)}">
    <span class="tree-label">${esc(d.name)}</span><span class="tree-val">${val}</span>
  </button></li>`;
}

function renderTree() {
  const groups = GROUP_ORDER.map((g) => ({
    key: g,
    devices: state.devices.filter((d) => groupOf(d) === g),
  })).filter((g) => g.devices.length);

  let inner = "";
  for (const g of groups) {
    // Subagrupar por "via" (receptor) manteniendo el orden del backend.
    const vias: (string | null)[] = [];
    for (const d of g.devices) if (!vias.includes(d.via)) vias.push(d.via);
    let items = "";
    for (const via of vias) {
      const ds = g.devices.filter((d) => d.via === via);
      if (via && g.key !== "bluetooth") {
        items += `<li><div class="tree-row branch">${icon(LINK_ICON[g.key])}<span class="tree-label">${esc(via)}</span></div><ul>${ds.map(treeLeaf).join("")}</ul></li>`;
      } else {
        items += ds.map(treeLeaf).join("");
      }
    }
    inner += `<li><div class="tree-row group"><span class="tree-label">${t(`group_${g.key}` as Key)}</span><span class="tree-val">${pad(g.devices.length)}</span></div><ul>${items}</ul></li>`;
  }
  if (!groups.length) inner = `<li><div class="tree-row is-dim"><span class="tree-label">${t("noDevices")}</span></div></li>`;

  const tool = (view: View, ic: IconName, label: string) =>
    `<li><button class="tree-row tool ${ui.view === view ? "is-selected" : ""}" data-view="${view}">${icon(ic)}<span class="tree-label">${label}</span></button></li>`;

  $("tree").innerHTML = `
    <div class="tree-head">${t("index")}</div>
    <ul class="tree-root">
      <li>
        <button class="tree-row root ${ui.view === "devices" && !ui.selected ? "is-selected" : ""}" data-view="devices">
          ${icon("root")}<span class="tree-label">${t("system")}</span><span class="tree-val">${pad(state.devices.length)}</span>
        </button>
        <ul>${inner}</ul>
      </li>
    </ul>
    <div class="tree-head">${t("tools")}</div>
    <ul class="tree-tools">
      ${tool("diag", "pulse", t("diag"))}
      ${tool("settings", "sliders", t("settings"))}
    </ul>`;
}

// ---------------------------------------------------------------------------
// Ficha técnica de un dispositivo
// ---------------------------------------------------------------------------

const SEGMENTS = 20;

function meter(d: Device): string {
  const p = percentOf(d);
  const filled = p == null ? 0 : Math.max(p > 0 ? 1 : 0, Math.round((p / 100) * SEGMENTS));
  const charging = d.battery?.charge === "charging";
  let segs = "";
  for (let i = 0; i < SEGMENTS; i++) {
    const on = i < filled;
    const pulse = charging && i === filled - 1 ? " pulse" : "";
    segs += `<i class="${on ? "on" : ""}${pulse}"></i>`;
  }
  return `<div class="meter" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${p ?? 0}">${segs}</div>
    <div class="meter-scale"><span>0</span><span>25</span><span>50</span><span>75</span><span>100</span></div>`;
}

function nameBlock(d: Device): string {
  if (ui.editing === d.fingerprint) {
    const custom = state.settings.deviceNames[d.fingerprint] != null;
    return `<form class="rename" data-rename="${esc(d.fingerprint)}">
      <input class="rename-input" name="name" value="${esc(d.name)}" maxlength="40" spellcheck="false" autocomplete="off" aria-label="${t("rename")}" />
      <div class="rename-actions">
        <span class="rename-hint">${t("renameHint")}</span>
        ${custom ? `<button type="button" class="btn btn-small" data-rename-auto="${esc(d.fingerprint)}">${t("renameAuto")}</button>` : ""}
        <button type="submit" class="btn btn-small">${t("save")}</button>
      </div>
    </form>`;
  }
  return `<h2 class="sheet-name">${esc(d.name)}<button class="rename-btn" data-rename-start="${esc(d.fingerprint)}" title="${t("rename")}" aria-label="${t("rename")}">${icon("edit")}</button></h2>`;
}

function sheet(d: Device, index: number): string {
  const st = d.status.state;
  const p = percentOf(d);
  const charge = d.battery?.charge ?? "unknown";

  const flags: string[] = [];
  if (d.low) flags.push(`<span class="flag flag-warn">${icon("warn")}${t("flagLow")}</span>`);
  if (charge === "charging") flags.push(`<span class="flag">${icon("bolt")}${t("flagCharging")}</span>`);
  if (charge === "full") flags.push(`<span class="flag">${icon("bolt")}${t("flagFull")}</span>`);
  if (d.battery?.approximate) flags.push(`<span class="flag flag-dim">${t("flagApprox")}</span>`);
  if (st === "offline") flags.push(`<span class="flag flag-dim">${p != null ? t("flagLast") : t("flagNoData")}</span>`);

  const value =
    st === "unavailable" || p == null
      ? `<span class="pct pct-none">--</span>`
      : `<span class="pct">${p}</span><span class="unit">%</span>`;

  const power =
    charge === "charging"
      ? t("powerCharging")
      : charge === "full"
        ? t("powerFull")
        : charge === "discharging"
          ? t("powerBattery")
          : "—";

  const statusLine =
    st === "online"
      ? `<span class="led led-on"></span>${t("online")}`
      : st === "offline"
        ? `<span class="led"></span>${t("offline")}`
        : `<span class="led led-warn"></span>${t("unavailable")}`;

  const code = [d.model, d.slot != null ? `SLOT ${pad(d.slot)}` : null].filter(Boolean).join(" · ");
  const detail = d.status.state === "unavailable" ? d.status.detail : null;

  return `<article class="sheet ${d.low ? "is-low" : ""} st-${st}" data-id="${esc(d.id)}">
    <header class="sheet-head">
      <span class="sheet-idx">${pad(index + 1)}</span>
      <span class="sheet-kind">${icon(KIND_ICON[d.kind])}${t(`kind_${d.kind}` as Key)}</span>
      <span class="sheet-vendor">${esc(d.vendor.toUpperCase())}</span>
    </header>
    <div class="sheet-title">
      ${nameBlock(d)}
      <div class="sheet-code">${esc(code)}<span class="name-src src-${d.nameSource}">${t(`src_${d.nameSource}` as Key)}</span></div>
    </div>
    <div class="readout">
      <div class="readout-value">${value}</div>
      <div class="readout-flags">${flags.join("")}</div>
    </div>
    ${meter(d)}
    ${detail ? `<div class="sheet-note">${icon("warn")}<span>${esc(detail)}</span></div>` : ""}
    <dl class="specs">
      <div><dt>${icon(LINK_ICON[d.connection])}${t("specLink")}</dt><dd>${t(`link_${d.connection}` as Key)}</dd></div>
      <div><dt>${icon("battery")}${t("specPower")}</dt><dd>${power}${d.battery?.voltageMv ? ` · ${(d.battery.voltageMv / 1000).toFixed(2)} V` : ""}</dd></div>
      <div><dt>${icon("clock")}${t("specRead")}</dt><dd>${d.lastRead ? clock(d.lastRead) : "—"}</dd></div>
    </dl>
    <footer class="sheet-foot">${statusLine}${
      d.configurable
        ? `<button class="btn btn-small" data-customize="${esc(d.id)}" title="${t("customizeTitle")}">${icon("sliders")}${t("customize")}</button>`
        : ""
    }<span class="sheet-prov">${esc(d.provider.toUpperCase())}</span></footer>
  </article>`;
}

function viewDevices(): string {
  const n = state.devices.length;
  const low = state.devices.filter((d) => d.low).length;
  const head = `<div class="view-head">
      <h1>${t("devices")} <span class="h-count">${pad(n)}</span></h1>
      <div class="view-meta">${low ? `<span class="meta-warn">${t("lowCount", { n: pad(low) })}</span>` : ""}<span>${t("threshold", { n: state.settings.lowThreshold })}</span></div>
    </div>`;
  if (!n) {
    return `${head}<div class="empty">
      <div class="empty-title">${t("noDevices")}</div>
      <ol class="empty-list">
        <li>${t("empty1")}</li>
        <li>${t("empty2a")} <button class="link" data-view="diag">${t("diag")}</button> ${t("empty2b")}</li>
        <li>${t("empty3a")} <button class="link" data-view="settings">${t("demoDevices")}</button>.</li>
      </ol>
    </div>`;
  }
  return `${head}<div class="sheets">${state.devices.map(sheet).join("")}</div>`;
}

// ---------------------------------------------------------------------------
// Personalizar un dispositivo (DPI, luces, reposo, audio…)
// ---------------------------------------------------------------------------

const SECTION_ORDER: ControlSection[] = ["pointer", "lighting", "audio", "power"];
const SWATCHES = ["#ffffff", "#ff0000", "#ff4f00", "#ffd000", "#00ff40", "#00e5ff", "#0050ff", "#b000ff", "#ff00a0"];

const detailDevice = (): Device | undefined => state.devices.find((d) => d.id === ui.detail);

/** ¿Se puede tocar este control ahora? */
function ctlLocked(c: Control): boolean {
  return (
    ui.ctlBusy != null ||
    detailDevice()?.status.state !== "online" ||
    (c.trust === "experimental" && !state.settings.experimentalControls)
  );
}

function ctlButtons(c: Control, options: { value: ControlValue; label: string }[], current: ControlValue | null): string {
  const dis = ctlLocked(c) ? "disabled" : "";
  return `<div class="segmented ctl-seg">${options
    .map(
      (o) =>
        `<button ${dis} class="${JSON.stringify(o.value) === JSON.stringify(current) ? "is-on" : ""}" data-ctl="${esc(c.id)}" data-value="${esc(JSON.stringify(o.value))}">${esc(o.label)}</button>`,
    )
    .join("")}</div>`;
}

function ctlWidget(c: Control): string {
  const dis = ctlLocked(c) ? "disabled" : "";
  const id = esc(c.id);
  switch (c.type) {
    case "toggle":
      return ctlButtons(c, [{ value: true, label: "ON" }, { value: false, label: "OFF" }], c.value);
    case "choice":
      if (c.options.length <= 5) return ctlButtons(c, c.options, c.value);
      return `<select class="ctl-select" ${dis} data-ctl="${id}">
        ${c.value == null ? `<option selected disabled>${t("ctlNotRead")}</option>` : ""}
        ${c.options.map((o) => `<option value="${o.value}" ${o.value === c.value ? "selected" : ""}>${esc(o.label)}</option>`).join("")}
      </select>`;
    case "range": {
      // Pocos niveles (brillo, velocidad): botones en vez de deslizador.
      if ((c.max - c.min) / c.step <= 8) {
        const levels = [];
        for (let v = c.min; v <= c.max; v += c.step) levels.push({ value: v, label: String(v) });
        return ctlButtons(c, levels, c.value);
      }
      const v = typeof c.value === "number" ? c.value : c.min;
      return `<div class="ctl-range">
        <div class="ctl-range-main">
          <input type="range" ${dis} data-ctl="${id}" min="${c.min}" max="${c.max}" step="${c.step}" value="${v}" aria-label="${esc(c.label)}" />
          <label class="ctl-number"><input type="number" ${dis} data-ctl="${id}" min="${c.min}" max="${c.max}" step="${c.step}" value="${c.value == null ? "" : v}" aria-label="${esc(c.label)}" /><span>${esc(c.unit)}</span></label>
        </div>
        ${c.presets.length ? ctlButtons(c, c.presets.map((p) => ({ value: p, label: String(p) })), c.value) : ""}
      </div>`;
    }
    case "color": {
      const v = typeof c.value === "string" ? c.value : "#ffffff";
      return `<div class="ctl-color">
        <label class="ctl-color-pick"><input type="color" ${dis} data-ctl="${id}" value="${esc(v)}" aria-label="${esc(c.label)}" /><span>${esc(v.toUpperCase())}</span></label>
        <div class="ctl-swatches">${SWATCHES.map(
          (s) => `<button ${dis} class="${s === v ? "is-on" : ""}" style="--sw:${s}" data-ctl="${id}" data-value="${esc(JSON.stringify(s))}" title="${s.toUpperCase()}" aria-label="${s}"></button>`,
        ).join("")}</div>
      </div>`;
    }
    case "bands": {
      const vals = Array.isArray(c.value) ? c.value : c.labels.map(() => 0);
      const sign = (n: number) => (n > 0 ? `+${n}` : String(n));
      return `<div class="ctl-bands-wrap">
        <div class="ctl-bands" data-bands="${id}">${c.labels
          .map(
            (l, i) => `<label class="ctl-band"><output>${c.value == null ? "?" : sign(vals[i] ?? 0)}</output>
            <input type="range" ${dis} data-band="${i}" min="${c.min}" max="${c.max}" step="1" value="${vals[i] ?? 0}" aria-label="${esc(l)} Hz" />
            <span>${esc(l)}</span></label>`,
          )
          .join("")}</div>
        ${ctlButtons(c, c.presets.map((p) => ({ value: p.values, label: p.label })), c.value)}
      </div>`;
    }
  }
}

function ctlRow(c: Control): string {
  const flags = [
    `<span class="flag ${c.trust === "experimental" ? "flag-warn" : "flag-dim"}" title="${t(`trustTitle_${c.trust}` as Key)}">${t(`trust_${c.trust}` as Key)}</span>`,
    c.volatile && state.settings.reapplyControls ? `<span class="flag flag-dim" title="${t("volatileTitle")}">${t("volatileFlag")}</span>` : "",
    c.value == null ? `<span class="flag flag-dim">${t("ctlNotRead")}</span>` : "",
  ].join("");
  return `<div class="set-row ctl-row ${c.type === "bands" ? "ctl-row-wide" : ""} ${ui.ctlBusy === c.id ? "is-busy" : ""}">
    <div class="set-label">${esc(c.label)}<span class="ctl-flags">${flags}</span>${c.hint ? `<small>${esc(c.hint)}</small>` : ""}</div>
    <div class="set-ctl">${ctlWidget(c)}</div>
  </div>`;
}

function viewDevice(): string {
  const d = detailDevice();
  if (!d) return viewDevices();
  const p = percentOf(d);
  const meta = [
    t(`kind_${d.kind}` as Key),
    d.vendor.toUpperCase(),
    d.model,
    p != null ? `${p} %` : null,
    d.status.state === "online" ? t("online") : d.status.state === "offline" ? t("offline") : t("unavailable"),
  ]
    .filter(Boolean)
    .map((x) => `<span>${esc(x!)}</span>`)
    .join("");
  const head = `<div class="view-head">
      <h1>${esc(d.name)}</h1>
      <div class="view-meta">
        <button class="btn" data-view="devices">${icon("back")}${t("backToDevices")}</button>
        <button class="btn ${ui.ctlLoading ? "is-busy" : ""}" data-action="reload-controls">${icon("scan")}${t("reread")}</button>
      </div>
    </div>
    <div class="detail-meta">${icon(KIND_ICON[d.kind])}${meta}</div>`;
  const note = (text: string) => `<div class="sheet-note">${icon("warn")}<span>${esc(text)}</span></div>`;

  let body = "";
  if (ui.ctlError) body += note(ui.ctlError);
  if (d.status.state !== "online") body += note(t("ctlOffline"));
  if (!ui.controls) {
    if (ui.ctlLoading) body += `<div class="empty"><div class="empty-title">${t("ctlLoading")}</div></div>`;
    return head + body;
  }
  const cs = ui.controls;
  body += cs.notes.map(note).join("");
  if (cs.controls.some((c) => c.trust === "experimental") && !state.settings.experimentalControls) {
    body += `<div class="sheet-note">${icon("warn")}<span>${t("expLocked")} <button class="link" data-view="settings">${t("expEnable")}</button></span></div>`;
  }
  if (!cs.controls.length) body += `<div class="empty"><div class="empty-title">${t("ctlNone")}</div></div>`;
  let n = 0;
  for (const section of SECTION_ORDER) {
    const list = cs.controls.filter((c) => c.section === section);
    if (!list.length) continue;
    body += `<section class="set-block">
      <h3 class="set-title"><span>${pad(++n)}</span>${t(`sec_${section}` as Key)}</h3>
      ${list.map(ctlRow).join("")}
    </section>`;
  }
  body += `<p class="hint ctl-source"><b>${t("ctlSource")}</b> ${esc(cs.source)}</p>`;
  return head + body;
}

async function loadControls() {
  const id = ui.detail;
  if (!id) return;
  ui.ctlLoading = true;
  ui.ctlError = null;
  render();
  try {
    const c = await backend.deviceControls(id);
    if (ui.detail !== id) return;
    ui.controls = c;
  } catch (e) {
    if (ui.detail !== id) return;
    ui.ctlError = String(e);
  }
  ui.ctlLoading = false;
  render();
}

function openDevice(id: string) {
  ui.view = "device";
  ui.selected = id;
  ui.detail = id;
  ui.controls = null;
  ui.ctlError = null;
  ui.ctlBusy = null;
  $("content").scrollTop = 0;
  void loadControls();
}

async function setControl(control: string, value: ControlValue) {
  const id = ui.detail;
  const c = ui.controls?.controls.find((x) => x.id === control);
  if (!id || !c || ctlLocked(c)) return;
  ui.ctlBusy = control;
  ui.ctlError = null;
  c.value = value; // respuesta inmediata; lo que cuenta es lo que conteste el dispositivo
  render();
  try {
    const next = await backend.setDeviceControl(id, control, value);
    if (ui.detail === id) ui.controls = next;
  } catch (e) {
    if (ui.detail === id) {
      ui.ctlError = String(e);
      // Lo que se enseñaba ya no es fiable: volver a leer el dispositivo.
      ui.ctlBusy = null;
      void backend.deviceControls(id).then(
        (fresh) => {
          if (ui.detail === id) {
            ui.controls = fresh;
            render();
          }
        },
        () => {},
      );
    }
  }
  if (ui.detail === id) {
    ui.ctlBusy = null;
    render();
  }
}

/** Cambios en deslizadores, campos numéricos, listas y selector de color. */
function bindControls() {
  const content = $("content");
  content.addEventListener("change", (ev) => {
    const el = ev.target as HTMLInputElement | HTMLSelectElement;
    const bands = el.closest<HTMLElement>("[data-bands]");
    if (bands) {
      const values = [...bands.querySelectorAll<HTMLInputElement>("input[data-band]")].map((i) => Number(i.value));
      void setControl(bands.dataset.bands!, values);
      return;
    }
    const control = el.dataset.ctl;
    if (!control) return;
    if (el instanceof HTMLInputElement && el.type === "color") void setControl(control, el.value.toLowerCase());
    else if (el.value !== "" && Number.isFinite(Number(el.value))) void setControl(control, Number(el.value));
  });
  // Mientras se arrastra: actualizar la cifra de al lado sin escribir en el dispositivo.
  content.addEventListener("input", (ev) => {
    const el = ev.target as HTMLInputElement;
    if (el.type !== "range") return;
    if (el.dataset.band != null) {
      const out = el.parentElement?.querySelector("output");
      const n = Number(el.value);
      if (out) out.textContent = n > 0 ? `+${n}` : String(n);
    } else {
      const num = el.closest(".ctl-range")?.querySelector<HTMLInputElement>('input[type="number"]');
      if (num) num.value = el.value;
    }
  });
}

// ---------------------------------------------------------------------------
// Ajustes
// ---------------------------------------------------------------------------

function seg<T extends string | number>(key: keyof Settings, options: [T, string][]): string {
  const cur = state.settings[key];
  return `<div class="segmented">${options
    .map(([v, l]) => `<button class="${cur === v ? "is-on" : ""}" data-set="${key}" data-value="${v}">${l}</button>`)
    .join("")}</div>`;
}

function toggle(key: keyof Settings, extra = ""): string {
  const on = state.settings[key] === true;
  return `<div class="ctl-group">${extra}<div class="segmented">
    <button class="${on ? "is-on" : ""}" data-set="${key}" data-value="true">ON</button>
    <button class="${!on ? "is-on" : ""}" data-set="${key}" data-value="false">OFF</button>
  </div></div>`;
}

function row(label: string, hint: string, control: string): string {
  return `<div class="set-row"><div class="set-label">${label}<small>${hint}</small></div><div class="set-ctl">${control}</div></div>`;
}

function viewSettings(): string {
  const s = state.settings;
  return `<div class="view-head"><h1>${t("settings")}</h1><div class="view-meta"><span>${isTauri ? t("savedIn") : t("browserMode")}</span></div></div>
  <section class="set-block">
    <h3 class="set-title"><span>01</span>${t("secPolling")}</h3>
    ${row(t("interval"), t("intervalHint"), seg("pollIntervalSecs", [[30, "30 S"], [60, "60 S"], [120, "2 MIN"], [300, "5 MIN"]]))}
    ${row(t("lowThreshold"), t("lowThresholdHint"), `<div class="stepper">
        <button data-step="-5" aria-label="−">−</button><output>${pad(s.lowThreshold)} %</output><button data-step="5" aria-label="+">+</button>
      </div>`)}
  </section>
  <section class="set-block">
    <h3 class="set-title"><span>02</span>${t("secSystem")}</h3>
    ${row(t("notifications"), t("notificationsHint"), toggle("notifications", `<button class="btn btn-small" data-action="test-notification">${t("test")}</button>`))}
    ${row(t("autostart"), t("autostartHint"), toggle("autostart"))}
  </section>
  <section class="set-block">
    <h3 class="set-title"><span>03</span>${t("secInterface")}</h3>
    ${row(t("language"), t("languageHint"), seg("language", [["system", t("langSystem")], ["es", "ESPAÑOL"], ["en", "ENGLISH"]]))}
    ${row(t("theme"), t("themeHint"), seg("theme", [["system", t("themeSystem")], ["light", t("themeLight")], ["dark", t("themeDark")]]))}
    ${row(t("showOffline"), t("showOfflineHint"), toggle("showOffline"))}
    ${row(t("demoDevices"), t("demoHint"), toggle("demoDevices"))}
  </section>
  <section class="set-block">
    <h3 class="set-title"><span>04</span>${t("secDevices")}</h3>
    ${row(t("reapply"), t("reapplyHint"), toggle("reapplyControls"))}
    ${row(t("experimental"), t("experimentalHint"), toggle("experimentalControls"))}
  </section>
  <section class="set-block">
    <h3 class="set-title"><span>05</span>${t("secAudio")}</h3>
    ${row(t("keepEq"), t("keepEqHint"), toggle("blacksharkKeepEq"))}
  </section>`;
}

async function saveSettings(patch: Partial<Settings>) {
  const next = { ...state.settings, ...patch };
  state.settings = next; // respuesta inmediata en la UI
  applyTheme();
  render();
  try {
    state.settings = await backend.updateSettings(next);
  } catch (e) {
    console.error(e);
  }
  applyTheme();
  render();
}

// ---------------------------------------------------------------------------
// Renombrar
// ---------------------------------------------------------------------------

function startRename(fingerprint: string) {
  ui.editing = fingerprint;
  render();
  const input = document.querySelector<HTMLInputElement>(".rename-input");
  input?.focus();
  input?.select();
}

function endRename() {
  ui.editing = null;
  if (ui.pending) {
    state = ui.pending;
    ui.pending = null;
  }
  render();
}

async function commitRename(fingerprint: string, name: string | null) {
  try {
    state.settings = await backend.renameDevice(fingerprint, name);
  } catch (e) {
    console.error(e);
  }
  endRename();
}

// ---------------------------------------------------------------------------
// Menú contextual propio (sustituye al de WebView2/Edge)
// ---------------------------------------------------------------------------

function closeMenu() {
  document.querySelector(".ctx-menu")?.remove();
}

function openDeviceMenu(d: Device, x: number, y: number) {
  closeMenu();
  const menu = document.createElement("div");
  menu.className = "ctx-menu";
  menu.setAttribute("role", "menu");
  const item = (action: string, ic: IconName, label: string) =>
    `<button role="menuitem" data-ctx="${action}">${icon(ic)}<span>${label}</span></button>`;
  menu.innerHTML = `
    <div class="ctx-title">${esc(d.name)}</div>
    ${d.configurable ? item("customize", "sliders", t("customize")) : ""}
    ${item("rename", "edit", t("rename"))}
    ${item("refresh", "scan", t("readNow"))}
    ${item("copy", "root", t("copyFingerprint"))}
    <div class="ctx-foot">${esc(d.fingerprint)}</div>`;
  document.body.appendChild(menu);
  // Que no se salga de la ventana.
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, innerWidth - r.width - 4)}px`;
  menu.style.top = `${Math.min(y, innerHeight - r.height - 4)}px`;
  menu.addEventListener("click", (ev) => {
    const action = (ev.target as HTMLElement).closest<HTMLElement>("[data-ctx]")?.dataset.ctx;
    if (!action) return;
    closeMenu();
    if (action === "customize") openDevice(d.id);
    else if (action === "rename") {
      ui.view = "devices";
      startRename(d.fingerprint);
    } else if (action === "refresh") void backend.refresh();
    else if (action === "copy") void navigator.clipboard?.writeText(d.fingerprint);
  });
  (menu.querySelector("button") as HTMLButtonElement | null)?.focus();
}

function bindContextMenu() {
  document.addEventListener("contextmenu", (ev) => {
    // Nunca el menú del navegador.
    ev.preventDefault();
    const el = ev.target as HTMLElement;
    if (el.closest(".ctx-menu")) return;
    const id =
      el.closest<HTMLElement>(".sheet")?.dataset.id ?? el.closest<HTMLElement>("[data-device]")?.dataset.device;
    const d = id ? state.devices.find((x) => x.id === id) : undefined;
    if (d && !ui.editing) openDeviceMenu(d, ev.clientX, ev.clientY);
    else closeMenu();
  });
  document.addEventListener("mousedown", (ev) => {
    if (!(ev.target as HTMLElement).closest(".ctx-menu")) closeMenu();
  });
  window.addEventListener("blur", closeMenu);
  window.addEventListener("resize", closeMenu);
  document.getElementById("content")?.addEventListener("scroll", closeMenu, { passive: true });
}

/** Atajos de navegador que no tienen sentido en una app (buscar, imprimir, recargar...). */
function blockBrowserShortcuts() {
  document.addEventListener(
    "keydown",
    (e) => {
      const k = e.key.toLowerCase();
      const ctrl = e.ctrlKey || e.metaKey;
      const blocked =
        (ctrl && ["f", "g", "p", "r", "u", "j", "s", "o", "n", "t", "h"].includes(k)) ||
        (ctrl && e.shiftKey && ["i", "c", "j", "r"].includes(k)) ||
        k === "f3" ||
        k === "f7" ||
        k === "f12" ||
        (k === "browserback" || k === "browserforward") ||
        (e.altKey && (k === "arrowleft" || k === "arrowright"));
      if (blocked) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    { capture: true },
  );
}

// ---------------------------------------------------------------------------
// Diagnóstico HID
// ---------------------------------------------------------------------------

async function loadHid() {
  ui.hidLoading = true;
  ui.hidError = null;
  render();
  try {
    ui.hid = await backend.hidInventory();
  } catch (e) {
    ui.hidError = String(e);
  }
  ui.hidLoading = false;
  render();
}

function viewDiag(): string {
  const rows = ui.hid ?? [];
  const claimed = rows.filter((r) => r.provider).length;
  const head = `<div class="view-head">
      <h1>${t("diag")} <span class="h-count">${pad(rows.length)}</span></h1>
      <div class="view-meta"><span>${t("withProvider", { n: pad(claimed) })}</span><button class="btn" data-action="rescan-hid">${icon("scan")}${t("rescan")}</button></div>
    </div>`;
  if (ui.hidError) return `${head}<div class="sheet-note">${icon("warn")}<span>${esc(ui.hidError)}</span></div>`;
  if (!ui.hid) return `${head}<div class="empty"><div class="empty-title">${ui.hidLoading ? t("enumerating") : "—"}</div></div>`;
  const body = rows
    .map(
      (r) => `<tr class="${r.provider ? "is-claimed" : ""}">
        <td>${hex(r.vid)}:${hex(r.pid)}</td>
        <td>${esc(r.vendor ?? "—")}</td>
        <td title="${esc(r.pathStr)}">${esc(r.product || "—")}${r.known ? `<span class="known"> · ${esc(r.known)}</span>` : ""}</td>
        <td class="num">${r.interface < 0 ? "—" : r.interface}</td>
        <td>${hex(r.usagePage)}:${hex(r.usage)}</td>
        <td>${r.provider ? esc(r.provider.toUpperCase()) : "—"}</td>
      </tr>`,
    )
    .join("");
  return `${head}
    <div class="table-wrap"><table class="hid">
      <thead><tr><th>VID:PID</th><th>${t("colVendor")}</th><th>${t("colProduct")}</th><th>IF</th><th>USAGE</th><th>PROVIDER</th></tr></thead>
      <tbody>${body}</tbody>
    </table></div>
    <p class="hint">${t("diagHint")}
    <code>cargo run --bin hid-list -- --probe</code> ${t("diagHintIn")} <code>src-tauri</code>.</p>`;
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

function renderContent() {
  const el = $("content");
  el.innerHTML =
    ui.view === "settings" ? viewSettings() : ui.view === "diag" ? viewDiag() : ui.view === "device" ? viewDevice() : viewDevices();
  // Ojo: no `data-view` (el manejador de clics lo trataría como un enlace de vista
  // y cada clic dentro del contenido repintaría todo, incluido el campo de renombrar).
  el.dataset.page = ui.view;
}

function renderStatus() {
  const n = state.devices.length;
  const online = state.devices.filter((d) => d.status.state === "online").length;
  const right = state.scanning
    ? `<span class="scan-led"></span>${t("scanning")}`
    : t("lastScan", { t: state.lastScan ? clock(state.lastScan) : "—", i: interval(state.settings.pollIntervalSecs) });
  $("statusbar").innerHTML = `
    <span>${t("statusDevices", { n: pad(n), m: pad(online) })}</span>
    <span class="status-right">${right}</span>`;
  $("btn-scan").classList.toggle("is-busy", state.scanning);
}

function render() {
  renderTree();
  renderContent();
  renderStatus();
  $("btn-settings").classList.toggle("is-on", ui.view === "settings");
}

function go(view: View) {
  ui.view = view;
  ui.detail = null;
  ui.controls = null;
  if (view !== "devices") ui.selected = null;
  render();
  $("content").scrollTop = 0;
  if (view === "diag" && !ui.hid && !ui.hidLoading) void loadHid();
}

function selectDevice(id: string) {
  // Con una ficha de ajustes abierta, el árbol cambia de dispositivo sin salir de ella.
  if (ui.view === "device") {
    const d = state.devices.find((x) => x.id === id);
    if (d?.configurable) return openDevice(id);
  }
  ui.view = "devices";
  ui.detail = null;
  ui.selected = id;
  render();
  const card = document.querySelector<HTMLElement>(`.sheet[data-id="${CSS.escape(id)}"]`);
  if (card) {
    card.scrollIntoView({ block: "nearest", behavior: "smooth" });
    card.classList.remove("flash");
    void card.offsetWidth;
    card.classList.add("flash");
  }
}

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

function bindChrome() {
  $("btn-min").innerHTML = icon("min");
  $("btn-close").innerHTML = icon("close");

  $("btn-scan").addEventListener("click", () => void backend.refresh());
  $("btn-settings").addEventListener("click", () => go(ui.view === "settings" ? "devices" : "settings"));
  $("btn-min").addEventListener("click", () => backend.minimize());
  $("btn-close").addEventListener("click", () => backend.close());

  document.body.addEventListener("click", (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>(
      "[data-view],[data-device],[data-set],[data-step],[data-action],[data-rename-start],[data-rename-auto],[data-customize],button[data-ctl]",
    );
    if (!el) return;
    const { view, device, set, value, step, action, renameStart, renameAuto, customize, ctl } = el.dataset;
    if (customize) openDevice(customize);
    else if (ctl && value != null) void setControl(ctl, JSON.parse(value) as ControlValue);
    else if (renameStart) startRename(renameStart);
    else if (renameAuto) void commitRename(renameAuto, null);
    else if (view) go(view as View);
    else if (device) selectDevice(device);
    else if (set && value != null) {
      const v: unknown = value === "true" ? true : value === "false" ? false : /^\d+$/.test(value) ? Number(value) : value;
      void saveSettings({ [set]: v } as Partial<Settings>);
    } else if (step) {
      const next = Math.min(50, Math.max(5, state.settings.lowThreshold + Number(step)));
      void saveSettings({ lowThreshold: next });
    } else if (action === "rescan-hid") void loadHid();
    else if (action === "reload-controls") void loadControls();
    else if (action === "test-notification") void backend.testNotification();
  });

  document.body.addEventListener("submit", (ev) => {
    const form = (ev.target as HTMLElement).closest<HTMLFormElement>("form[data-rename]");
    if (!form) return;
    ev.preventDefault();
    const name = (form.elements.namedItem("name") as HTMLInputElement).value;
    void commitRename(form.dataset.rename!, name);
  });

  // Atajos: F5 escanear, Esc cancelar edición o volver a dispositivos.
  document.addEventListener("keydown", (e) => {
    if (e.key === "F5") {
      e.preventDefault();
      void backend.refresh();
    } else if (e.key === "Escape") {
      if (document.querySelector(".ctx-menu")) closeMenu();
      else if (ui.editing) endRename();
      else if (ui.view !== "devices") go("devices");
    }
  });

  darkQuery.addEventListener("change", applyTheme);
}

function onState(s: AppState) {
  // Mientras se edita un nombre no se repinta (se perdería lo escrito).
  if (ui.editing) {
    ui.pending = s;
    return;
  }
  const langChanged = s.lang !== state.lang;
  const before = detailDevice()?.status.state;
  state = s;
  applyTheme();
  if (langChanged) applyLang();
  if (ui.view === "device") {
    const d = detailDevice();
    if (!d) return go("devices"); // se ha desconectado
    // No repintar la ficha mientras se usa un control (se perdería el arrastre):
    // solo el árbol y la barra de estado, salvo que cambie el estado del dispositivo.
    if (d.status.state === before && !langChanged) {
      renderTree();
      renderStatus();
      return;
    }
    if (d.status.state === "online" && before !== "online") void loadControls();
  }
  render();
}

async function main() {
  backend = await createBackend();
  state = await backend.getState();
  applyTheme();
  bindChrome();
  bindContextMenu();
  bindControls();
  blockBrowserShortcuts();
  applyLang();
  render();
  // El backend empuja el estado tras cada sondeo o evento: aquí no hay temporizadores.
  backend.onState(onState);
}

void main();
