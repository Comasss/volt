// Puente con el backend. Dentro de Tauri usa IPC + eventos (sin sondeo desde JS).
// En un navegador normal (`npm run dev` sin Tauri) usa datos simulados, para poder
// iterar la interfaz sin compilar Rust.

import type { AppState, Device, HidRow, Settings } from "./types";

export interface Backend {
  getState(): Promise<AppState>;
  refresh(): Promise<void>;
  updateSettings(s: Settings): Promise<Settings>;
  hidInventory(): Promise<HidRow[]>;
  renameDevice(fingerprint: string, name: string | null): Promise<Settings>;
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
