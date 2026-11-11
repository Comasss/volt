// Puente con el backend. Dentro de Tauri usa IPC + eventos (sin sondeo desde JS).
// En un navegador normal (`npm run dev` sin Tauri) usa datos simulados, para poder
// iterar la interfaz sin compilar Rust.

import type { AppState, Control, ControlValue, Device, DeviceControls, HidRow, Settings } from "./types";

export interface Backend {
  getState(): Promise<AppState>;
  refresh(): Promise<void>;
  updateSettings(s: Settings): Promise<Settings>;
  hidInventory(): Promise<HidRow[]>;
  renameDevice(fingerprint: string, name: string | null): Promise<Settings>;
  /** Lee del dispositivo lo que se puede personalizar y su valor actual. */
  deviceControls(device: string): Promise<DeviceControls>;
  /** Escribe un control; devuelve la lista releída del dispositivo. */
  setDeviceControl(device: string, control: string, value: ControlValue): Promise<DeviceControls>;
  testNotification(): Promise<void>;
  onState(cb: (s: AppState) => void): void;
  minimize(): void;
  close(): void;
}

export const isTauri = "__TAURI_INTERNALS__" in window;

export async function createBackend(): Promise<Backend> {
  return isTauri ? tauriBackend() : browserBackend();
}

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  return {
    getState: () => invoke<AppState>("get_state"),
    refresh: () => invoke("refresh_now"),
    updateSettings: (settings) => invoke<Settings>("update_settings", { settings }),
    hidInventory: () => invoke<HidRow[]>("hid_inventory"),
    renameDevice: (fingerprint, name) => invoke<Settings>("rename_device", { fingerprint, name }),
    deviceControls: (device) => invoke<DeviceControls>("device_controls", { device }),
    setDeviceControl: (device, control, value) =>
      invoke<DeviceControls>("set_device_control", { device, control, value }),
    testNotification: () => invoke("test_notification"),
    onState: (cb) => void listen<AppState>("volt://state", (e) => cb(e.payload)),
    minimize: () => void win.minimize(),
    // Destruye la ventana (y el webview); la app sigue en la bandeja.
    close: () => void win.close(),
  };
}

// ---------------------------------------------------------------------------
// Simulación en navegador
// ---------------------------------------------------------------------------

function browserBackend(): Backend {
  const now = Date.now();
  const dev = (d: Partial<Device> & Pick<Device, "id" | "name" | "kind" | "connection">): Device => ({
    fingerprint: d.id,
    nameSource: "device",
    provider: "mock",
    vendor: "Demo",
    model: "0000:0000",
    via: null,
    slot: null,
    status: { state: "online" },
    battery: { percent: 50, approximate: false, charge: "discharging", voltageMv: null },
    lastRead: now,
    low: false,
    configurable: ["mouse", "keyboard", "headset"].includes(d.kind) && d.status?.state !== "unavailable",
    ...d,
  });
  let settings: Settings = {
    pollIntervalSecs: 60,
    lowThreshold: 15,
    notifications: true,
    theme: "system",
    demoDevices: true,
    autostart: false,
    blacksharkKeepEq: true,
    deviceNames: {},
    language: "system",
    showOffline: false,
    reapplyControls: true,
    experimentalControls: false,
    deviceControls: {},
  };
  // Controles de ejemplo por tipo de dispositivo (los valores viven en memoria).
  const level = { type: "range" as const, min: 0, max: 4, step: 1, unit: "", presets: [] as number[] };
  const base = { hint: null, trust: "reference", volatile: false } as const;
  const templates: Record<string, Control[]> = {
    mouse: [
      { ...base, id: "mode", section: "pointer", label: "QUIÉN MANDA", type: "choice", value: 1, volatile: true,
        hint: "Con la memoria del ratón manda el perfil guardado en él. El DPI y la tasa suelen necesitar el modo software.",
        options: [{ value: 1, label: "MEMORIA DEL RATÓN" }, { value: 2, label: "SOFTWARE (VOLT)" }] },
      { ...base, id: "dpi", section: "pointer", label: "DPI", type: "range", value: 1600, volatile: true,
        hint: "Sensibilidad del sensor. Los límites y el paso son los que anuncia el propio ratón.",
        min: 100, max: 32000, step: 50, unit: "DPI", presets: [400, 800, 1200, 1600, 3200] },
      { ...base, id: "rate", section: "pointer", label: "TASA DE SONDEO", type: "choice", value: 1000, volatile: true,
        options: [125, 250, 500, 1000, 2000, 4000, 8000].map((hz) => ({ value: hz, label: hz >= 1000 ? `${hz / 1000} KHZ` : `${hz} HZ` })) },
    ],
    keyboard: [
      { ...base, id: "light.effect", section: "lighting", label: "EFECTO", type: "choice", value: 2, trust: "sibling",
        hint: "Los nombres son los del Attack Shark X87; en tu teclado el orden puede ser otro.",
        options: ["FIJO", "RESPIRACIÓN", "ARCOÍRIS", "DESTELLO", "GOTAS", "RUEDA ARCOÍRIS", "ONDAS", "ESTRELLAS"].map((l, i) => ({ value: i + 1, label: `${String(i + 1).padStart(2, "0")} · ${l}` })) },
      { ...base, ...level, id: "light.brightness", section: "lighting", label: "BRILLO", value: 3, trust: "sibling", hint: "0 apaga las luces. Se guarda por efecto." },
      { ...base, ...level, id: "light.speed", section: "lighting", label: "VELOCIDAD", value: 2, trust: "sibling" },
      { ...base, id: "light.palette", section: "lighting", label: "COLOR DE LA PALETA", type: "choice", value: 0, trust: "sibling",
        options: [...[1, 2, 3, 4, 5, 6, 7].map((n) => ({ value: n - 1, label: String(n) })), { value: 7, label: "MULTICOLOR" }] },
      { ...base, id: "light.color", section: "lighting", label: "COLOR PERSONALIZADO", type: "color", value: "#ff4f00", trust: "experimental",
        hint: "Sustituye el color seleccionado de la paleta de este efecto." },
      { ...base, id: "power.sleep", section: "power", label: "REPOSO Y APAGADO DE LUCES", type: "choice", value: 300, trust: "sibling",
        hint: "Tiempo sin pulsar teclas hasta que el teclado se duerme y apaga las luces.",
        options: [[0, "NUNCA"], [60, "1 MIN"], [120, "2 MIN"], [300, "5 MIN"], [600, "10 MIN"], [1200, "20 MIN"]].map(([value, label]) => ({ value: value as number, label: label as string })) },
    ],
    headset: [
      { ...base, id: "eq.bands", section: "audio", label: "ECUALIZADOR PERSONALIZADO", type: "bands", value: [-3, -3, -4, 0, 5, 5, 4, 1, 0, -1], volatile: true,
        hint: "10 bandas. Al cambiarlas el auricular pasa al preset personalizado.",
        labels: ["31", "63", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"], min: -6, max: 6, unit: "dB",
        presets: [
          { label: "PLANO", values: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
          { label: "JUEGO", values: [-3, -3, -4, 0, 5, 5, 4, 1, 0, -1] },
          { label: "MÚSICA", values: [2, 2, 1, 1, 2, 3, 3, 3, 1, 0] },
          { label: "CINE", values: [4, 4, 3, 0, -3, -1, 3, 5, 2, 1] },
        ] },
      { ...base, id: "audio.enhancement", section: "audio", label: "MEJORA DE AUDIO", type: "toggle", value: null, volatile: true },
      { ...base, id: "audio.sidetone", section: "audio", label: "TONO LATERAL", type: "toggle", value: false, trust: "experimental" },
    ],
  };
  const controlValues = new Map<string, ControlValue>();
  const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const controlsOf = (id: string): DeviceControls => {
    const d = devices.find((x) => x.id === id);
    if (!d) throw "El dispositivo ya no está conectado";
    if (d.status.state !== "online") throw "Desconectado o en reposo";
    const controls = (templates[d.kind] ?? []).map((c) =>
      controlValues.has(`${id}/${c.id}`) ? ({ ...c, value: controlValues.get(`${id}/${c.id}`)! } as Control) : c,
    );
    const mode = controls.find((c) => c.id === "mode")?.value;
    return {
      controls,
      source: "Dispositivo simulado (modo navegador): los cambios solo se guardan en memoria.",
      notes: mode === 1 ? ["El ratón está usando su memoria interna: puede ignorar el DPI y la tasa que pongas aquí hasta que cambies a modo software."] : [],
    };
  };
  const browserLang = (): "es" | "en" =>
    settings.language !== "system"
      ? settings.language
      : navigator.language.toLowerCase().startsWith("es")
        ? "es"
        : "en";
  const autoNames = new Map<string, [string, Device["nameSource"]]>();
  let devices: Device[] = [
    dev({
      id: "logitech:c54d:1",
      provider: "logitech",
      vendor: "Logitech",
      name: "PRO X 2",
      model: "046D:C54D",
      kind: "mouse",
      connection: "receiver",
      via: "LIGHTSPEED C54D",
      slot: 1,
      battery: { percent: 92, approximate: false, charge: "discharging", voltageMv: null },
    }),
    dev({
      id: "mock:hs02",
      name: "HS-02 HEADSET",
      model: "0000:H502",
      kind: "headset",
      connection: "receiver",
      via: "DEMO RECEIVER R1",
      slot: 2,
      battery: { percent: 12, approximate: false, charge: "discharging", voltageMv: null },
      low: true,
    }),
    dev({
      id: "mock:tb01",
      name: "TB-01 TRACKBALL",
      model: "0000:T801",
      kind: "mouse",
      connection: "wired",
      status: { state: "unavailable", detail: "Ocupado: bloqueado por la app del fabricante" },
      battery: null,
      lastRead: null,
    }),
    dev({
      id: "mock:kb75",
      name: "KB-75 MECHANICAL",
      model: "DE:M0:00:00:00:75",
      kind: "keyboard",
      connection: "bluetooth",
      via: "BLUETOOTH LE",
      battery: { percent: 46, approximate: false, charge: "charging", voltageMv: null },
    }),
    dev({
      id: "mock:pd01",
      name: "PD-01 CONTROLLER",
      model: "DE:M0:00:00:00:01",
      kind: "gamepad",
      connection: "bluetooth",
      via: "BLUETOOTH CLÁSICO",
      status: { state: "offline" },
      battery: { percent: 64, approximate: false, charge: "unknown", voltageMv: null },
      lastRead: now - 47 * 60_000,
    }),
  ];
  let listener: ((s: AppState) => void) | null = null;
  let lastScan = now;
  const named = (d: Device): Device => {
    if (!autoNames.has(d.fingerprint)) autoNames.set(d.fingerprint, [d.name, d.nameSource]);
    const custom = settings.deviceNames[d.fingerprint];
    const [name, nameSource] = custom ? [custom.toUpperCase(), "user" as const] : autoNames.get(d.fingerprint)!;
    return { ...d, name, nameSource };
  };
  const snapshot = (scanning = false): AppState => ({
    devices: (settings.demoDevices ? devices : devices.filter((d) => d.provider !== "mock"))
      .filter((d) => settings.showOffline || d.status.state !== "offline")
      .map(named),
    lastScan,
    scanning,
    settings,
    lang: browserLang(),
  });
  const tick = () => {
    listener?.(snapshot(true));
    setTimeout(() => {
      lastScan = Date.now();
      devices = devices.map((d) => {
        if (d.status.state !== "online" || !d.battery?.percent) return d;
        const delta = d.battery.charge === "charging" ? 2 : -1;
        const percent = Math.max(1, Math.min(100, d.battery.percent + delta));
        return {
          ...d,
          lastRead: lastScan,
          battery: { ...d.battery, percent },
          low: d.battery.charge !== "charging" && percent <= settings.lowThreshold,
        };
      });
      listener?.(snapshot());
    }, 400);
  };
  return {
    getState: async () => snapshot(),
    refresh: async () => tick(),
    updateSettings: async (s) => {
      settings = { ...s };
      devices = devices.map((d) => ({
        ...d,
        low:
          !!d.battery?.percent &&
          d.battery.charge !== "charging" &&
          d.battery.percent <= settings.lowThreshold,
      }));
      listener?.(snapshot());
      return settings;
    },
    renameDevice: async (fingerprint, name) => {
      const names = { ...settings.deviceNames };
      if (name && name.trim()) names[fingerprint] = name.trim();
      else delete names[fingerprint];
      settings = { ...settings, deviceNames: names };
      listener?.(snapshot());
      return settings;
    },
    deviceControls: async (id) => {
      await wait(350);
      return controlsOf(id);
    },
    setDeviceControl: async (id, control, value) => {
      await wait(350);
      const c = controlsOf(id).controls.find((x) => x.id === control);
      if (!c) throw "Este dispositivo no tiene ese ajuste";
      if (c.trust === "experimental" && !settings.experimentalControls)
        throw "Control experimental: actívalos en Ajustes para poder usarlo";
      // El DPI se ajusta al paso válido, como hace el dispositivo real.
      const v = c.type === "range" && typeof value === "number" ? Math.round(value / c.step) * c.step : value;
      controlValues.set(`${id}/${control}`, v);
      return controlsOf(id);
    },
    testNotification: async () => {
      console.info("notificación de prueba (modo navegador)");
    },
    hidInventory: async () => [
      { pathStr: "", vid: 0x046d, pid: 0xc54d, usagePage: 0xff00, usage: 1, interface: 2, product: "USB Receiver", manufacturer: "Logitech", serial: "", bus: "Usb", vendor: "Logitech", known: "LIGHTSPEED RECEIVER", provider: "logitech" },
      { pathStr: "", vid: 0x046d, pid: 0xc54d, usagePage: 0xff00, usage: 2, interface: 2, product: "USB Receiver", manufacturer: "Logitech", serial: "", bus: "Usb", vendor: "Logitech", known: "LIGHTSPEED RECEIVER", provider: "logitech" },
      { pathStr: "", vid: 0x1532, pid: 0x0555, usagePage: 0xff00, usage: 1, interface: 3, product: "Razer BlackShark V2 Pro 2.4", manufacturer: "Razer", serial: "", bus: "Usb", vendor: "Razer", known: "BLACKSHARK V2 PRO", provider: null },
    ],
    onState: (cb) => {
      listener = cb;
      setInterval(tick, 8000);
    },
    minimize: () => {},
    close: () => {},
  };
}
