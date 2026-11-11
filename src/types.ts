// Espejo de los tipos serializados por el backend (src-tauri/src/model.rs, manager.rs).

export type DeviceKind = "mouse" | "keyboard" | "headset" | "gamepad" | "other";
export type Connection = "receiver" | "wired" | "bluetooth" | "virtual";
export type ChargeState = "discharging" | "charging" | "full" | "unknown";

export interface BatteryReading {
  percent: number | null;
  approximate: boolean;
  charge: ChargeState;
  voltageMv: number | null;
}

export type DeviceStatus =
  | { state: "online" }
  | { state: "offline" }
  | { state: "unavailable"; detail: string };

export type NameSource = "device" | "database" | "probable" | "usb_ids" | "user" | "generic";

export interface Device {
  id: string;
  fingerprint: string;
  nameSource: NameSource;
  provider: string;
  vendor: string;
  name: string;
  model: string;
  kind: DeviceKind;
  connection: Connection;
  via: string | null;
  slot: number | null;
  status: DeviceStatus;
  battery: BatteryReading | null;
  lastRead: number | null;
  low: boolean;
  /** Hay ajustes que personalizar (DPI, luces, reposo, audio…). */
  configurable: boolean;
}

// ---- Controles configurables (src-tauri/src/controls.rs) ----

export type ControlSection = "pointer" | "lighting" | "power" | "audio";
/** Cuánto se puede confiar en el protocolo del control. */
export type ControlTrust = "reference" | "sibling" | "experimental";
/** Booleano, número, color `#rrggbb` o lista de bandas. */
export type ControlValue = boolean | number | string | number[];

interface ControlBase {
  id: string;
  section: ControlSection;
  label: string;
  hint: string | null;
  /** `null` si el ajuste se puede escribir pero no leer. */
  value: ControlValue | null;
  trust: ControlTrust;
  /** El dispositivo lo olvida al apagarse; Volt lo guarda y lo reaplica. */
  volatile: boolean;
}

export type Control = ControlBase &
  (
    | { type: "range"; min: number; max: number; step: number; unit: string; presets: number[] }
    | { type: "choice"; options: { value: number; label: string }[] }
    | { type: "toggle" }
    | { type: "color" }
    | {
        type: "bands";
        labels: string[];
        min: number;
        max: number;
        unit: string;
        presets: { label: string; values: number[] }[];
      }
  );

export interface DeviceControls {
  controls: Control[];
  /** De dónde sale el protocolo y qué se ha verificado. */
  source: string;
  notes: string[];
}

export type Theme = "system" | "light" | "dark";

export interface Settings {
  pollIntervalSecs: number;
  lowThreshold: number;
  notifications: boolean;
  theme: Theme;
  demoDevices: boolean;
  autostart: boolean;
  blacksharkKeepEq: boolean;
  deviceNames: Record<string, string>;
  language: "system" | "es" | "en";
  showOffline: boolean;
  reapplyControls: boolean;
  experimentalControls: boolean;
  /** Lo mantiene el backend: huella → (control → valor). */
  deviceControls: Record<string, Record<string, ControlValue>>;
}

export interface AppState {
  devices: Device[];
  lastScan: number | null;
  scanning: boolean;
  settings: Settings;
  /** Idioma efectivo resuelto por el backend. */
  lang: "es" | "en";
}

export interface HidRow {
  pathStr: string;
  vid: number;
  pid: number;
  usagePage: number;
  usage: number;
  interface: number;
  product: string;
  manufacturer: string;
  serial: string;
  bus: string;
  vendor: string | null;
  known: string | null;
  provider: string | null;
}
