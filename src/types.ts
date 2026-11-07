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
