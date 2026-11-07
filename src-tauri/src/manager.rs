//! Gestor: coordina los providers desde un único hilo que duerme entre sondeos.
//!
//! - Nada de bucles activos: el hilo espera en `recv_timeout` hasta el siguiente
//!   sondeo o hasta que llega una orden (refrescar, ajustes, diagnóstico, aviso de
//!   Windows de que se ha enchufado algo, evento empujado por un dispositivo).
//! - Cada provider va envuelto en `catch_unwind`: un fallo en uno no afecta al resto.
//! - Dispositivos que avisan solos (cable conectado, batería): un hilo por
//!   dispositivo bloqueado en la lectura (0 % de CPU) que manda `Push` al cambiar.
//! - El resultado se publica como evento Tauri (`volt://state`) y en la bandeja.

use crate::device_ids;
use crate::hid::{HidContext, HidEntry};
use crate::model::{
    now_ms, BatteryReading, DeviceInfo, DeviceKind, DeviceSnapshot, DeviceStatus,
};
use crate::provider::{DeviceProvider, ListenEvent, ListenSpec, PollContext, ProviderError};
use crate::providers::{self, mock::MockProvider};
use crate::settings::Settings;
use crate::{devwatch, names, notify, synapse, tray};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const EVENT_STATE: &str = "volt://state";

/// Se olvidan los dispositivos que no se ven desde hace este tiempo.
const FORGET_AFTER_MS: u64 = 12 * 3600 * 1000;
/// Tras un aviso de enchufado/desenchufado, esperar a que termine la enumeración.
const SETTLE_AFTER_CHANGE: Duration = Duration::from_millis(1500);
/// Un valor empujado más reciente que esto se usa en lugar de volver a preguntar.
const PUSH_FRESH: Duration = Duration::from_secs(20);
/// Sondeos seguidos sin respuesta antes de dar por apagado un dispositivo cuyo
/// provider no es fiable (con 60 s de intervalo, unos 3 minutos).
const UNRELIABLE_MISSES: u32 = 3;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePayload {
    pub devices: Vec<DeviceSnapshot>,
    pub last_scan: Option<u64>,
    pub scanning: bool,
    pub settings: Settings,
    /// Idioma efectivo ("es"/"en") tras resolver "system".
    pub lang: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HidRow {
    #[serde(flatten)]
    pub entry: HidEntry,
    pub vendor: Option<&'static str>,
    /// Nombre de la tabla de Volt o de usb.ids.
    pub known: Option<String>,
    pub provider: Option<&'static str>,
}

pub enum Command {
    /// Sondear ya.
    Refresh,
    /// Cambiaron los ajustes: recalcular avisos, nombres e intervalo sin tocar el
    /// hardware (salvo que cambie algo que lo requiera).
    Reconfigure,
    /// Listado HID crudo para la vista de diagnóstico.
    HidInventory(Sender<Vec<HidRow>>),
    /// Windows avisa de que apareció o desapareció una interfaz HID.
    DevicesChanged,
    /// Un dispositivo informó por sí mismo de un cambio.
    Push(String, BatteryReading),
    /// Un dispositivo avisó (o dejó de informar) de que se ha apagado.
    Gone(String),
}

/// Estado compartido con los comandos Tauri.
pub struct Shared {
    state: Mutex<StatePayload>,
    tx: Mutex<Option<Sender<Command>>>,
    /// `--mock` en la línea de comandos: demo activado sin tocar los ajustes.
    pub force_demo: bool,
}

impl Shared {
    pub fn new(settings: Settings, force_demo: bool) -> Self {
        let lang = crate::i18n::set(&settings.language);
        Self {
            state: Mutex::new(StatePayload {
                settings,
                lang: lang.code().into(),
                ..Default::default()
            }),
            tx: Mutex::new(None),
            force_demo,
        }
    }

    pub fn snapshot(&self) -> StatePayload {
        self.state.lock().unwrap().clone()
    }

    pub fn settings(&self) -> Settings {
        self.state.lock().unwrap().settings.clone()
    }

    pub fn set_settings(&self, s: Settings) {
        let lang = crate::i18n::set(&s.language);
        let mut st = self.state.lock().unwrap();
        st.lang = lang.code().into();
        st.settings = s;
    }

    pub fn send(&self, cmd: Command) -> bool {
        self.tx
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|tx| tx.send(cmd).is_ok())
    }

    fn sender(&self) -> Option<Sender<Command>> {
        self.tx.lock().unwrap().clone()
    }

    fn demo_enabled(&self) -> bool {
        self.force_demo || self.state.lock().unwrap().settings.demo_devices
    }
}

struct Tracked {
    /// Tal cual lo da el provider (el nombre final se resuelve al publicar).
    info: DeviceInfo,
    status: DeviceStatus,
    battery: Option<BatteryReading>,
    last_read: Option<u64>,
    last_seen: u64,
    alerted: bool,
    /// Sondeos seguidos sin respuesta.
    offline_streak: u32,
    /// Se sabe con certeza que está apagado/desconectado (evento o provider fiable).
    gone: bool,
}

impl Tracked {
    /// ¿Se enseña en la lista? Los desconectados se ocultan salvo que se pida verlos.
    /// Las lecturas perdidas de providers poco fiables ya se absorben al sondear
    /// (`UNRELIABLE_MISSES`): aquí "Offline" significa apagado de verdad.
    fn visible(&self, show_offline: bool) -> bool {
        self.status != DeviceStatus::Offline || show_offline
    }
}

/// Hilo de escucha de un dispositivo que empuja su estado.
struct Listener {
    alive: Arc<AtomicBool>,
    /// Último valor recibido (aunque no haya cambiado) y cuándo.
    latest: Arc<Mutex<Option<(Instant, BatteryReading)>>>,
}

impl Listener {
    fn fresh(&self) -> Option<BatteryReading> {
        if !self.alive.load(Ordering::Relaxed) {
            return None;
        }
        let latest = self.latest.lock().unwrap();
        latest
            .as_ref()
            .filter(|(at, _)| at.elapsed() < PUSH_FRESH)
            .map(|(_, r)| r.clone())
    }
}

pub fn spawn(app: AppHandle, shared: Arc<Shared>) {
    let (tx, rx) = mpsc::channel();
    *shared.tx.lock().unwrap() = Some(tx.clone());
    // Cable enchufado, receptor conectado...: leer enseguida en vez de esperar al sondeo.
    let tx_dev = Mutex::new(tx);
    devwatch::start(move || {
        let _ = tx_dev.lock().map(|t| t.send(Command::DevicesChanged));
    });
    std::thread::Builder::new()
        .name("volt-poller".into())
        .spawn(move || Poller::new(app, shared).run(rx))
        .expect("no se pudo crear el hilo de sondeo");
}

struct Poller {
    app: AppHandle,
    shared: Arc<Shared>,
    hid: Option<HidContext>,
    providers: Vec<Box<dyn DeviceProvider>>,
    mock: MockProvider,
    devices: BTreeMap<String, Tracked>,
    listeners: HashMap<String, Listener>,
    demo_was: bool,
    keep_eq_was: bool,
}

impl Poller {
    fn new(app: AppHandle, shared: Arc<Shared>) -> Self {
        let hid = HidContext::new()
            .map_err(|e| log::error!("hidapi no disponible: {e}"))
            .ok();
        Self {
            app,
            shared,
            hid,
            providers: providers::all(),
            mock: MockProvider::new(),
            devices: BTreeMap::new(),
            listeners: HashMap::new(),
            demo_was: false,
            keep_eq_was: false,
        }
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(self.shared.settings().poll_interval_secs as u64)
    }

    fn run(mut self, rx: Receiver<Command>) {
        let mut last_poll = Instant::now();
        let mut next_poll = last_poll; // primer sondeo inmediato
        // El primer sondeo y los pedidos (Escanear, enchufar/desenchufar) leen todo ya.
        let mut forced = true;
        loop {
            if Instant::now() >= next_poll {
                self.poll(forced);
                forced = false;
                last_poll = Instant::now();
                next_poll = last_poll + self.interval();
            }
            match rx.recv_timeout(next_poll.saturating_duration_since(Instant::now())) {
                // Un doble clic o una petición llegada durante el sondeo no repite la lectura.
                Ok(Command::Refresh) if last_poll.elapsed() > Duration::from_millis(1500) => {
                    next_poll = Instant::now();
                    forced = true;
                }
                Ok(Command::Refresh) => {}
                Ok(Command::DevicesChanged) => {
                    // Varios avisos seguidos (cada colección HID avisa) → una sola lectura.
                    next_poll = next_poll.min(Instant::now() + SETTLE_AFTER_CHANGE);
                    forced = true;
                }
                Ok(Command::Push(id, reading)) => self.apply_push(&id, reading),
                Ok(Command::Gone(id)) => {
                    if let Some(t) = self.devices.get_mut(&id) {
                        log::info!("{}: apagado o fuera de alcance", t.info.name);
                        t.status = DeviceStatus::Offline;
                        t.gone = true;
                        self.publish(false);
                    }
                }
                Ok(Command::Reconfigure) => {
                    next_poll = last_poll + self.interval();
                    let keep_eq = self.shared.settings().blackshark_keep_eq;
                    if self.shared.demo_enabled() != self.demo_was || keep_eq != self.keep_eq_was {
                        next_poll = Instant::now();
                    } else {
                        self.publish(false);
                    }
                }
                Ok(Command::HidInventory(reply)) => {
                    let _ = reply.send(self.hid_inventory());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn apply_push(&mut self, id: &str, reading: BatteryReading) {
        let Some(t) = self.devices.get_mut(id) else {
            return;
        };
        log::info!("{}: evento {:?} {:?}", t.info.name, reading.charge, reading.percent);
        t.battery = Some(reading);
        t.last_read = Some(now_ms());
        t.status = DeviceStatus::Online;
        t.gone = false;
        t.offline_streak = 0;
        self.publish(false);
    }

    fn poll(&mut self, forced: bool) {
        self.set_scanning();
        let started = Instant::now();
        let now = now_ms();
        let demo = self.shared.demo_enabled();
        self.demo_was = demo;
        self.listeners.retain(|_, l| l.alive.load(Ordering::Relaxed));

        match self.hid.as_mut() {
            Some(h) => h.refresh(),
            None => self.hid = HidContext::new().ok(),
        }
        let keep_eq = self.shared.settings().blackshark_keep_eq;
        self.keep_eq_was = keep_eq;
        let ctx = PollContext {
            hid: self.hid.as_ref(),
            razer_keep_eq: keep_eq,
            forced,
        };
        let tx = self.shared.sender();
        let mut seen = HashSet::new();
        let mut cycle = Cycle {
            ctx: &ctx,
            devices: &mut self.devices,
            listeners: &mut self.listeners,
            seen: &mut seen,
            tx: tx.as_ref(),
            now,
        };
        for p in self.providers.iter_mut() {
            cycle.poll_provider(p.as_mut());
        }
        if demo {
            cycle.poll_provider(&mut self.mock);
        }

        // Lo que ya no aparece en el sistema (receptor quitado, cable fuera...) se
        // quita de la lista enseguida.
        self.devices.retain(|id, t| {
            seen.contains(id)
                && (demo || t.info.provider != "mock")
                && now.saturating_sub(t.last_seen) < FORGET_AFTER_MS
        });

        // Con un auricular Razer presente y Synapse abierto, esperar (sin sondeo) a
        // que Synapse se cierre para reajustar el EQ del auricular al momento.
        let razer_headset = self.devices.values().any(|t| {
            t.info.provider == "razer" && t.info.kind == DeviceKind::Headset && seen.contains(&t.info.id)
        });
        if keep_eq && razer_headset {
            if let Some(pid) = synapse::find() {
                let shared = self.shared.clone();
                synapse::watch_exit(pid, move || {
                    shared.send(Command::Refresh);
                });
            }
        }
        log::debug!(
            "sondeo: {} dispositivos, {} escuchas, en {:?}",
            self.devices.len(),
            self.listeners.len(),
            started.elapsed()
        );
        self.publish(true);
    }

    fn set_scanning(&self) {
        let payload = {
            let mut st = self.shared.state.lock().unwrap();
            st.scanning = true;
            st.clone()
        };
        let _ = self.app.emit(EVENT_STATE, &payload);
    }

    /// Recalcula avisos y nombres, actualiza el estado compartido, emite el evento
    /// y la bandeja.
    fn publish(&mut self, scanned: bool) {
        let settings = self.shared.settings();
        let threshold = settings.low_threshold;
        let mut alerts = Vec::new();

        let mut list: Vec<DeviceSnapshot> = self
            .devices
            .values_mut()
            .filter(|t| t.visible(settings.show_offline))
            .map(|t| {
                let (name, name_source) = names::resolve(
                    &t.info.fingerprint,
                    &t.info.name,
                    t.info.name_from_device,
                    names::parse_vid_pid(&t.info.model),
                    &settings.device_names,
                );
                let pct = t.battery.as_ref().and_then(|b| b.percent);
                let charging = t.battery.as_ref().is_some_and(|b| b.is_charging());
                let low = !charging && pct.is_some_and(|p| p <= threshold);
                if low && t.status == DeviceStatus::Online && !t.alerted {
                    t.alerted = true;
                    alerts.push((name.clone(), pct.unwrap_or(0)));
                }
                // Histéresis: se rearma al cargar o al subir 5 puntos por encima.
                if t.alerted && (charging || pct.is_some_and(|p| p > threshold.saturating_add(5))) {
                    t.alerted = false;
                }
                let mut info = t.info.clone();
                info.name = name;
                DeviceSnapshot {
                    info,
                    status: t.status.clone(),
                    battery: t.battery.clone(),
                    last_read: t.last_read,
                    low,
                    name_source,
                }
            })
            .collect();
        list.sort_by(|a, b| {
            (a.info.connection, &a.info.via, &a.info.name).cmp(&(
                b.info.connection,
                &b.info.via,
                &b.info.name,
            ))
        });

        if settings.notifications {
            for (name, pct) in alerts {
                notify::low_battery(&name, pct);
            }
        }

        let payload = {
            let mut st = self.shared.state.lock().unwrap();
            st.devices = list;
            st.scanning = false;
            if scanned {
                st.last_scan = Some(now_ms());
            }
            st.clone()
        };
        let _ = self.app.emit(EVENT_STATE, &payload);
        tray::update(&self.app, &payload.devices);
    }

    fn hid_inventory(&mut self) -> Vec<HidRow> {
        if let Some(h) = self.hid.as_mut() {
            h.refresh();
        }
        let Some(h) = self.hid.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<HidRow> = h
            .devices
            .iter()
            .map(|e| HidRow {
                vendor: device_ids::vendor_name(e.vid),
                known: device_ids::lookup(e.vid, e.pid)
                    .map(|k| k.name.to_string())
                    .or_else(|| names::usb_name(e.vid, e.pid)),
                provider: self
                    .providers
                    .iter()
                    .find(|p| p.claims_hid(e.vid, e.pid, e.usage_page, e.usage))
                    .map(|p| p.id()),
                entry: e.clone(),
            })
            .collect();
        rows.sort_by_key(|r| {
            (
                r.entry.vid,
                r.entry.pid,
                r.entry.interface,
                r.entry.usage_page,
                r.entry.usage,
            )
        });
        rows
    }
}

/// Lo que comparte un ciclo de sondeo entre providers.
struct Cycle<'a, 'b> {
    ctx: &'a PollContext<'b>,
    devices: &'a mut BTreeMap<String, Tracked>,
    listeners: &'a mut HashMap<String, Listener>,
    seen: &'a mut HashSet<String>,
    tx: Option<&'a Sender<Command>>,
    now: u64,
}

impl Cycle<'_, '_> {
    fn poll_provider(&mut self, p: &mut dyn DeviceProvider) {
        let ctx = self.ctx;
        let found = match catch_unwind(AssertUnwindSafe(|| p.discover(ctx))) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                log::warn!("{}: discover: {e}", p.id());
                Vec::new()
            }
            Err(_) => {
                log::error!("{}: pánico en discover", p.id());
                Vec::new()
            }
        };
        for info in found {
            // Si su escucha ya trae un valor reciente, no hace falta preguntar.
            let pushed = self.listeners.get(&info.id).and_then(Listener::fresh);
            let result = match pushed {
                Some(r) => Ok(Ok(r)),
                None => catch_unwind(AssertUnwindSafe(|| p.battery(ctx, &info))),
            };
            self.seen.insert(info.id.clone());
            let now = self.now;
            let t = self.devices.entry(info.id.clone()).or_insert_with(|| Tracked {
                info: info.clone(),
                status: DeviceStatus::Offline,
                battery: None,
                last_read: None,
                last_seen: now,
                alerted: false,
                offline_streak: 0,
                gone: false,
            });
            t.info = info;
            t.last_seen = now;
            match result {
                Ok(Ok(r)) => {
                    t.last_read = Some(r.measured_at.unwrap_or(now));
                    t.battery = Some(r);
                    t.status = DeviceStatus::Online;
                    t.offline_streak = 0;
                    t.gone = false;
                }
                Ok(Err(ProviderError::Offline { .. }))
                    if !p.offline_is_reliable()
                        && t.status == DeviceStatus::Online
                        && t.offline_streak + 1 < UNRELIABLE_MISSES =>
                {
                    // Lectura perdida de un provider poco fiable (p. ej. el teclado
                    // Compx dormitando): se queda en línea con su último valor en vez de
                    // parpadear entre "en línea" y "desconectado".
                    t.offline_streak += 1;
                    log::debug!("{}: sin respuesta ({} seguidas), se mantiene", t.info.name, t.offline_streak);
                }
                Ok(Err(ProviderError::Offline { last })) => {
                    t.offline_streak = t.offline_streak.saturating_add(1);
                    t.gone = t.gone || p.offline_is_reliable();
                    if let Some(l) = last {
                        let at = l.measured_at.unwrap_or(now);
                        if t.last_read.is_none_or(|prev| at > prev) {
                            t.last_read = Some(at);
                            t.battery = Some(l);
                        }
                    }
                    t.status = DeviceStatus::Offline;
                }
                Ok(Err(e)) => {
                    log::info!("{}: {}: {e}", p.id(), t.info.name);
                    t.status = DeviceStatus::Unavailable(e.to_string());
                }
                Err(_) => {
                    log::error!("{}: pánico leyendo {}", p.id(), t.info.name);
                    t.status = DeviceStatus::Unavailable("error interno del provider".into());
                }
            }

            // Arrancar la escucha si el provider la ofrece y aún no está activa.
            let id = t.info.id.clone();
            if !self.listeners.contains_key(&id) {
                if let (Some(spec), Some(hid), Some(tx)) =
                    (p.listener(&t.info, t.battery.as_ref()), ctx.hid, self.tx)
                {
                    let initial = t.battery.clone();
                    if let Some(l) = start_listener(hid, id.clone(), spec, tx.clone(), initial) {
                        log::info!("escuchando eventos de {}", t.info.name);
                        self.listeners.insert(id, l);
                    }
                }
            }
        }
        let _ = catch_unwind(AssertUnwindSafe(|| p.release()));
    }
}

/// Abre la colección y deja un hilo bloqueado leyendo. Solo despierta cuando el
/// dispositivo manda algo, y solo molesta al gestor si el valor cambia. Termina
/// sola cuando el dispositivo desaparece (la lectura falla).
fn start_listener(
    hid: &HidContext,
    id: String,
    spec: ListenSpec,
    tx: Sender<Command>,
    initial: Option<BatteryReading>,
) -> Option<Listener> {
    let dev = hid.open(&spec.path).ok()?;
    let alive = Arc::new(AtomicBool::new(true));
    let latest: Arc<Mutex<Option<(Instant, BatteryReading)>>> = Arc::new(Mutex::new(None));
    let (alive_t, latest_t) = (alive.clone(), latest.clone());
    let mut parser = spec.parser;
    let silence = spec.silence;
    let spawned = std::thread::Builder::new()
        .name(format!("volt-listen-{id}"))
        .spawn(move || {
            let mut buf = [0u8; 128];
            let mut last = initial;
            // Con `silence`, la lectura se corta a ese tiempo para detectar que el
            // dispositivo ha dejado de informar (apagado); sin él, espera indefinida.
            let timeout = silence.map(|d| d.as_millis() as i32).unwrap_or(-1);
            let mut silent = false;
            loop {
                let event = match dev.read_timeout(&mut buf, timeout) {
                    Ok(0) if silence.is_some() => {
                        if silent {
                            continue;
                        }
                        silent = true;
                        last = None; // al volver, la primera lectura se publica
                        Some(ListenEvent::Gone)
                    }
                    Ok(0) => None,
                    Ok(n) => parser(&buf[..n]),
                    Err(_) => break,
                };
                let cmd = match event {
                    Some(ListenEvent::Reading(r)) => {
                        silent = false;
                        *latest_t.lock().unwrap() = Some((Instant::now(), r.clone()));
                        if last.as_ref() == Some(&r) {
                            continue;
                        }
                        last = Some(r.clone());
                        Command::Push(id.clone(), r)
                    }
                    Some(ListenEvent::Gone) => {
                        *latest_t.lock().unwrap() = None;
                        Command::Gone(id.clone())
                    }
                    Some(ListenEvent::Refresh) => Command::Refresh,
                    None => continue,
                };
                if tx.send(cmd).is_err() {
                    break;
                }
            }
            alive_t.store(false, Ordering::Relaxed);
        });
    spawned.ok().map(|_| Listener { alive, latest })
}
