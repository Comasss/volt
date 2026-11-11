//! Provider Compx: receptores 2.4 GHz con VID `3554` que usan muchos teclados
//! chinos (Redragon, AULA, MCHOSE, Womier...).
//!
//! Protocolo (ingeniería inversa comunitaria: sanshoku#36, AulaMonitor, BatteryMonitor;
//! capturado en un AULA F75 con el mismo receptor `3554:FA09`):
//!
//! ```text
//! colección vendor FF02:0002, informe 0x13 de 20 bytes en ambos sentidos
//! suma = byte bajo de la suma de los 19 bytes anteriores
//!
//! variante A  13 4A 00 … <suma>       →  13 4A|80 01 00 02 <nivel> <estado> … <suma>
//! variante B  13 05 01 … <suma>       →  13 05 …  y después  13 0A 01 00 04 05 <nivel> <estado> … <suma>
//!             nivel 1..100 · estado 0x01 = batería, 0x10 = cable (el nivel marca 100 siempre)
//! ```
//!
//! El teclado Redragon probado aquí (receptor FA09) contesta con la variante B.
//!
//! Receptor compartido: FA09 puede tener emparejados un teclado y un ratón (aquí, un
//! Attack Shark X7). La trama de batería no lleva índice de dispositivo y el receptor
//! ignora el argumento de 0x05. Con el ratón apagado sigue contestando con el mismo
//! valor, así que la batería que se ve por este canal es la del **teclado**. El ratón
//! no expone la suya por aquí.
//!
//! Solo se habla con los PID de la tabla (`Protocol::Compx`): el mismo VID lo usan
//! ratones con otros comandos.

use super::compx_cfg::{self as cfg, CfgError, LedConfig};
use crate::controls::{choice, Control, DeviceControls, Kind, Section, Trust, Value};
use crate::device_ids::{self, Protocol};
use crate::i18n::tr;
use crate::hid::{interface_key, short_hash, HidEntry};
use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{
    Capabilities, DeviceProvider, ListenEvent, ListenSpec, PollContext, ProviderError,
};
use hidapi::HidDevice;
use std::collections::HashMap;
use std::ffi::CString;
use std::thread::sleep;
use std::time::{Duration, Instant};

const VID: u16 = 0x3554;
const USAGE_PAGE: u16 = 0xFF02;
const USAGE: u16 = 0x0002;
const REPORT_ID: u8 = 0x13;
const LEN: usize = 20;
const CMD_BATTERY: u8 = 0x4A;
const STATE_CABLE: u8 = 0x10;

/// Cada consulta despierta el enlace del teclado (y algunos encienden sus luces):
/// la batería cambia despacio, así que se le pregunta como mucho cada 5 minutos
/// salvo que el usuario pida escanear o se enchufe/desenchufe algo.
const MIN_QUERY_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    /// 0x4A (AULA F75...).
    A,
    /// Sesión 0x05 + trama 0x0A (Redragon UCAL Pro, MCHOSE...).
    B,
}

pub struct CompxProvider {
    paths: HashMap<String, CString>,
    /// Variante de protocolo que ya contestó en cada receptor.
    variants: HashMap<String, Variant>,
    /// ID de modelo por receptor (se conserva aunque el teclado se duerma).
    models: HashMap<String, String>,
    /// Última lectura buena: cuándo y cuál (unix ms incluido para "LECTURA").
    cache: HashMap<String, (Instant, BatteryReading)>,
    /// Lecturas hechas en `discover` para devolver en `battery`.
    results: HashMap<String, Result<BatteryReading, ProviderError>>,
    /// Tabla de paletas de cada teclado (512 bytes). Leerla son 35 paquetes por
    /// radio, así que se lee una vez y se actualiza con lo que se escribe.
    tables: HashMap<String, Vec<u8>>,
}

impl CompxProvider {
    pub fn new() -> Self {
        Self {
            paths: HashMap::new(),
            variants: HashMap::new(),
            models: HashMap::new(),
            cache: HashMap::new(),
            results: HashMap::new(),
            tables: HashMap::new(),
        }
    }

    fn open(&self, ctx: &PollContext, device: &DeviceInfo) -> Result<HidDevice, ProviderError> {
        let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
        let path = self.paths.get(&device.id).ok_or(ProviderError::NoResponse)?;
        hid.open(path)
            .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))
    }
}

fn cfg_err(e: CfgError) -> ProviderError {
    match e {
        CfgError::Io(s) => ProviderError::Io(s),
        CfgError::Asleep => ProviderError::Offline { last: None },
        CfgError::Timeout => ProviderError::NoResponse,
        CfgError::Bad(s) => ProviderError::Protocol(s.into()),
    }
}

/// Bloque de configuración, solo si tiene la forma que se sabe editar.
fn read_valid_led(dev: &HidDevice) -> Result<LedConfig, ProviderError> {
    let led = cfg::read_led(dev).map_err(cfg_err)?;
    if !led.is_valid() {
        log::warn!("compx: bloque de configuración sin las marcas esperadas: {:02x?}", &led.0[..]);
        return Err(ProviderError::Unsupported(
            tr(
                "este teclado guarda su configuración en un formato que Volt no conoce; no se toca nada",
                "this keyboard stores its settings in a layout Volt does not know; nothing is written",
            )
            .into(),
        ));
    }
    Ok(led)
}

/// Nombres de los efectos según la herramienta del fabricante del Attack Shark
/// X87. En otros teclados de la familia el orden puede variar: por eso llevan el
/// número delante.
const EFFECTS: [(&str, &str); 18] = [
    ("FIJO", "STATIC"),
    ("RESPIRACIÓN", "BREATHING"),
    ("ARCOÍRIS", "RAINBOW"),
    ("DESTELLO", "FLASH AWAY"),
    ("GOTAS", "RAINDROPS"),
    ("RUEDA ARCOÍRIS", "RAINBOW WHEEL"),
    ("ONDAS", "RIPPLES"),
    ("ESTRELLAS", "STARS"),
    ("SOMBRA", "SHADOW"),
    ("SERPIENTE", "SNAKE"),
    ("NEÓN", "NEON"),
    ("REACTIVO", "REACTIVE"),
    ("ONDA SENOIDAL", "SINE WAVE"),
    ("BARRIDO", "SCANNING"),
    ("MOLINO", "WINDMILL"),
    ("CASCADA", "WATERFALL"),
    ("FLORECER", "BLOSSOMING"),
    ("TORMENTA", "STORM"),
];

/// Tiempos de reposo que ofrece la app del fabricante (segundos); 0 = nunca.
const SLEEP_PRESETS: [u32; 11] = [0, 30, 60, 90, 120, 180, 240, 300, 600, 900, 1200];

fn sleep_label(secs: u32) -> String {
    match secs {
        0 => tr("NUNCA", "NEVER").into(),
        s if s < 60 => format!("{s} S"),
        s if s % 60 == 0 => format!("{} MIN", s / 60),
        s => format!("{:.1} MIN", s as f32 / 60.0),
    }
}

fn level(v: &Value, max: u8) -> Result<u8, ProviderError> {
    v.as_int()
        .filter(|n| (0..=max as i64).contains(n))
        .map(|n| n as u8)
        .ok_or_else(|| ProviderError::Protocol("valor fuera de rango".into()))
}

fn checksum(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |a, x| a.wrapping_add(*x))
}

/// Variante B (otros firmwares del mismo receptor): tras una trama de sesión
/// `13 05 01`, el receptor empuja `13 0A 01 00 04 05 <nivel> <estado>`.
const CMD_SESSION: u8 = 0x05;
const CMD_PUSH: u8 = 0x0A;

fn frame(cmd: u8, arg: u8) -> [u8; LEN] {
    let mut p = [0u8; LEN];
    p[0] = REPORT_ID;
    p[1] = cmd;
    p[2] = arg;
    p[LEN - 1] = checksum(&p[..LEN - 1]);
    p
}

fn request() -> [u8; LEN] {
    frame(CMD_BATTERY, 0)
}

/// (nivel, estado) si `r` es una respuesta válida de cualquiera de las dos variantes.
fn parse(r: &[u8]) -> Option<(u8, u8)> {
    if r.len() < 8 || r[0] != REPORT_ID {
        return None;
    }
    // Las tramas completas llevan suma; las publicadas recortadas (8 bytes) no.
    let sum_ok = r.len() < LEN || checksum(&r[..LEN - 1]) == r[LEN - 1];
    match r[1] & 0x7F {
        CMD_BATTERY if r.len() >= LEN && sum_ok => Some((r[5], r[6])),
        // El receptor manda más tramas 0x0A (estado, teclas, luces...): solo la de
        // batería tiene la cabecera `01 00 04 05`. Sin esta comprobación, la escucha
        // en segundo plano tomaba otras tramas por batería y la cifra parpadeaba.
        CMD_PUSH
            if sum_ok
                && r[2..6] == [0x01, 0x00, 0x04, 0x05]
                && (1..=100).contains(&r[6]) =>
        {
            Some((r[6], r[7]))
        }
        _ => None,
    }
}

/// ID de modelo de la respuesta de identificación (0x05):
/// `13 05 01 00 0a <6 bytes de modelo> …`. Es el mismo ID que usan los drivers del
/// fabricante para saber qué teclado hay detrás de un receptor compartido
/// (p. ej. `030000000115` = MCHOSE G75 Pro, `0300000000ba` = Redragon UCAL Pro).
fn parse_identity(r: &[u8]) -> Option<String> {
    if r.len() < 11 || r[0] != REPORT_ID || r[1] & 0x7F != CMD_SESSION {
        return None;
    }
    let id = &r[5..11];
    if id.iter().all(|&b| b == 0) {
        return None;
    }
    Some(id.iter().map(|b| format!("{b:02x}")).collect())
}

/// Lectura de batería decodificada: (nivel, estado, vino por la variante B).
type Reply = (u8, u8, bool);

/// Batería y, si aparece, el ID de modelo.
///
/// Con la variante ya conocida solo se manda su comando (dos intentos: el primero
/// a veces solo despierta el enlace). Si no se conoce: B (que además identifica el
/// modelo), A y otra vez B.
fn ask(
    dev: &HidDevice,
    variant: Option<Variant>,
    model: &mut Option<String>,
) -> Result<Option<Reply>, ProviderError> {
    // Respuestas viejas fuera.
    let mut buf = [0u8; 64];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}

    let (a, b) = (request(), frame(CMD_SESSION, 0x01));
    let tries: &[[u8; LEN]] = match variant {
        Some(Variant::A) => &[a, a],
        Some(Variant::B) => &[b, b],
        None => &[b, a, b],
    };
    for (attempt, pkt) in tries.iter().enumerate() {
        if attempt > 0 {
            sleep(Duration::from_millis(300));
        }
        log::trace!("compx tx {:02x?}", &pkt[..4]);
        dev.write(pkt)
            .map_err(|e| ProviderError::Io(e.to_string()))?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match dev.read_timeout(&mut buf, left.as_millis().max(1) as i32) {
                Ok(0) => {}
                Ok(n) => {
                    log::trace!("compx rx {:02x?}", &buf[..n]);
                    if let Some(id) = parse_identity(&buf[..n]) {
                        *model = Some(id);
                    }
                    if let Some((level, state)) = parse(&buf[..n]) {
                        return Ok(Some((level, state, buf[1] & 0x7F == CMD_PUSH)));
                    }
                }
                Err(e) => return Err(ProviderError::Io(e.to_string())),
            }
        }
    }
    Ok(None)
}

fn to_reading(reply: Option<(u8, u8)>) -> Result<BatteryReading, ProviderError> {
    match reply {
        Some((_, STATE_CABLE)) => Ok(BatteryReading {
            percent: None,
            approximate: false,
            charge: ChargeState::Charging,
            voltage_mv: None,
            measured_at: None,
        }),
        Some((level @ 1..=100, _)) => Ok(BatteryReading::percent(level, ChargeState::Discharging)),
        // Sin respuesta o nivel fuera de rango: teclado apagado o dormido.
        _ => Err(ProviderError::Offline { last: None }),
    }
}

/// Para la escucha en segundo plano: tramas que el receptor empuja solo.
pub fn parse_push(r: &[u8]) -> Option<BatteryReading> {
    parse(r).and_then(|(l, s)| to_reading(Some((l, s))).ok())
}

impl DeviceProvider for CompxProvider {
    fn id(&self) -> &'static str {
        "compx"
    }

    fn claims_hid(&self, vid: u16, pid: u16, up: u16, usage: u16) -> bool {
        vid == VID
            && up == USAGE_PAGE
            && usage == USAGE
            && device_ids::lookup(vid, pid).is_some_and(|k| k.protocol == Protocol::Compx)
    }

    /// La consulta se hace aquí (y no en `battery`) porque la misma respuesta trae
    /// el ID de modelo, que hace falta para poner el nombre.
    fn discover(&mut self, ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        self.paths.clear();
        self.results.clear();
        let Some(hid) = ctx.hid else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let candidates: Vec<&HidEntry> = hid
            .devices
            .iter()
            .filter(|e| self.claims_hid(e.vid, e.pid, e.usage_page, e.usage))
            .collect();
        for e in candidates {
            let known = device_ids::lookup(e.vid, e.pid);
            let id = format!("compx:{:04x}:{}", e.pid, short_hash(&interface_key(e)));

            // Lectura reciente y sondeo no pedido: no se molesta al teclado.
            let cached = self
                .cache
                .get(&id)
                .filter(|(at, _)| !ctx.forced && at.elapsed() < MIN_QUERY_INTERVAL)
                .map(|(_, r)| r.clone());
            let result = match (cached, hid.open(&e.path)) {
                (Some(r), _) => Ok(r),
                (None, Ok(dev)) => {
                    let mut model = self.models.get(&id).cloned();
                    let reply = ask(&dev, self.variants.get(&id).copied(), &mut model);
                    if let Some(m) = model {
                        self.models.insert(id.clone(), m);
                    }
                    let reading = reply.and_then(|r| {
                        if let Some((_, _, push)) = r {
                            let v = if push { Variant::B } else { Variant::A };
                            self.variants.insert(id.clone(), v);
                        }
                        to_reading(r.map(|(l, s, _)| (l, s)))
                    });
                    if let Ok(r) = &reading {
                        // Se guarda con su hora real para que "LECTURA" la muestre.
                        let mut r = r.clone();
                        r.measured_at = Some(crate::model::now_ms());
                        self.cache.insert(id.clone(), (Instant::now(), r));
                    }
                    reading
                }
                (None, Err(err)) => Err(ProviderError::Busy(format!("no se pudo abrir ({err})"))),
            };

            let fingerprint = match self.models.get(&id) {
                Some(m) => format!("compx:{m}"),
                None => format!("compx:{:04x}", e.pid),
            };
            out.push(DeviceInfo {
                id: id.clone(),
                fingerprint,
                provider: "compx",
                vendor: "Compx".into(),
                name: crate::i18n::tr("TECLADO 2.4G", "2.4G KEYBOARD").into(),
                name_from_device: false,
                model: format!("{VID:04X}:{:04X}", e.pid),
                kind: known.map(|k| k.kind).unwrap_or(DeviceKind::Keyboard),
                connection: Connection::Receiver,
                via: Some(format!("COMPX 2.4G {:04X}", e.pid)),
                slot: None,
            });
            self.results.insert(id.clone(), result);
            self.paths.insert(id, e.path.clone());
        }
        Ok(out)
    }

    fn battery(
        &mut self,
        _ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        self.results
            .remove(&device.id)
            .unwrap_or(Err(ProviderError::NoResponse))
    }

    fn listener(&self, device: &DeviceInfo, _current: Option<&BatteryReading>) -> Option<ListenSpec> {
        let path = self.paths.get(&device.id)?.clone();
        Some(ListenSpec {
            path,
            parser: Box::new(|r| parse_push(r).map(ListenEvent::Reading)),
            silence: None,
        })
    }

    fn capabilities(&self, _device: &DeviceInfo) -> Capabilities {
        Capabilities {
            lighting: true,
            sleep: true,
            ..Capabilities::BATTERY_ONLY
        }
    }

    fn controls(&mut self, ctx: &PollContext, device: &DeviceInfo) -> Result<DeviceControls, ProviderError> {
        let dev = self.open(ctx, device)?;
        let led = read_valid_led(&dev)?;
        log::debug!("compx: configuración {:02x?}", &led.0[..]);
        let mut notes = Vec::new();
        let mut controls = Vec::new();

        let effect = led.effect();
        let mut options: Vec<(i64, String)> = EFFECTS
            .iter()
            .enumerate()
            .map(|(i, (es, en))| (i as i64 + 1, format!("{:02} · {}", i + 1, tr(es, en))))
            .collect();
        if effect == cfg::EFFECT_CUSTOM {
            options.push((effect as i64, tr("21 · POR TECLA", "21 · PER KEY").into()));
        } else if !(1..=18).contains(&effect) {
            options.push((effect as i64, format!("{effect:02} · ?")));
        }
        controls.push(
            Control::new("light.effect", Section::Lighting, tr("EFECTO", "EFFECT"), choice(options), Some(Value::Int(effect as i64)))
                .trust(Trust::Sibling)
                .hint(tr(
                    "Los nombres son los del Attack Shark X87; en tu teclado el orden puede ser otro.",
                    "Names come from the Attack Shark X87; the order may differ on your keyboard.",
                )),
        );

        let level_kind = || Kind::Range { min: 0, max: cfg::MAX_LEVEL as i64, step: 1, unit: "", presets: vec![] };
        if let (Some(b), Some(sp), Some(color)) = (led.brightness(), led.speed(), led.color()) {
            controls.push(
                Control::new("light.brightness", Section::Lighting, tr("BRILLO", "BRIGHTNESS"), level_kind(), Some(Value::Int(b as i64)))
                    .trust(Trust::Sibling)
                    .hint(tr("0 apaga las luces. Se guarda por efecto.", "0 turns the lights off. Stored per effect.")),
            );
            controls.push(
                Control::new("light.speed", Section::Lighting, tr("VELOCIDAD", "SPEED"), level_kind(), Some(Value::Int(sp as i64)))
                    .trust(Trust::Sibling),
            );
            let mut palette: Vec<(i64, String)> = (0..cfg::PALETTE_COLORS as i64).map(|i| (i, format!("{}", i + 1))).collect();
            palette.push((cfg::COLOR_RANDOM as i64, tr("MULTICOLOR", "MULTICOLOUR").into()));
            controls.push(
                Control::new("light.palette", Section::Lighting, tr("COLOR DE LA PALETA", "PALETTE COLOUR"), choice(palette), Some(Value::Int(color as i64)))
                    .trust(Trust::Sibling)
                    .hint(tr(
                        "Cada efecto tiene 7 colores guardados en el teclado.",
                        "Each effect has 7 colours stored in the keyboard.",
                    )),
            );

            // La tabla de paletas: solo hace falta para enseñar y editar el color.
            if !self.tables.contains_key(&device.id) {
                match cfg::read_rgb_table(&dev) {
                    Ok(t) => {
                        self.tables.insert(device.id.clone(), t);
                    }
                    Err(e) => {
                        log::info!("compx: no se pudo leer la tabla de paletas: {e:?}");
                        notes.push(
                            tr(
                                "No se pudo leer la tabla de colores del teclado: el color personalizado no está disponible.",
                                "Could not read the keyboard colour table: custom colour is unavailable.",
                            )
                            .into(),
                        );
                    }
                }
            }
            if let (Some(table), Some(slot)) = (self.tables.get(&device.id), led.slot()) {
                // Con «multicolor» se enseña (y se editaría) el último color.
                let index = color.min(cfg::PALETTE_COLORS - 1);
                let o = cfg::palette_offset(slot, index);
                controls.push(
                    Control::new(
                        "light.color",
                        Section::Lighting,
                        tr("COLOR PERSONALIZADO", "CUSTOM COLOUR"),
                        Kind::Color,
                        Some(Value::rgb(table[o], table[o + 1], table[o + 2])),
                    )
                    .trust(Trust::Experimental)
                    .hint(tr(
                        "Sustituye el color seleccionado de la paleta de este efecto. Reescribe la tabla de colores del teclado, algo que no se ha verificado por radio en ningún modelo: si el resultado no es el esperado, un restablecimiento de fábrica del teclado recupera las paletas.",
                        "Replaces the selected palette colour of this effect. It rewrites the keyboard colour table, which has not been verified over the air on any model: if the result is wrong, a factory reset of the keyboard restores the palettes.",
                    )),
                );
            }
        } else {
            notes.push(
                tr(
                    "El efecto activo no está en la tabla conocida: brillo, velocidad y color no se pueden ajustar hasta elegir otro.",
                    "The active effect is not in the known table: brightness, speed and colour cannot be set until you pick another one.",
                )
                .into(),
            );
        }

        let secs = led.sleep_secs();
        let mut times: Vec<u32> = SLEEP_PRESETS.to_vec();
        if !times.contains(&secs) {
            times.push(secs);
            times[1..].sort_unstable();
        }
        controls.push(
            Control::new(
                "power.sleep",
                Section::Power,
                tr("REPOSO Y APAGADO DE LUCES", "SLEEP AND LIGHTS OFF"),
                choice(times.into_iter().map(|s| (s as i64, sleep_label(s)))),
                Some(Value::Int(secs as i64)),
            )
            .trust(Trust::Sibling)
            .hint(tr(
                "Tiempo sin pulsar teclas hasta que el teclado se duerme y apaga las luces (solo por 2.4G).",
                "Idle time before the keyboard sleeps and turns its lights off (2.4G only).",
            )),
        );

        Ok(DeviceControls {
            controls,
            source: tr(
                "Protocolo de attack_shark_x87_rs (MIT), verificado por su autor en un Attack Shark X87 con este mismo receptor. Sin probar en este teclado: Volt lee su configuración, cambia solo el byte del ajuste y comprueba que se ha guardado.",
                "Protocol from attack_shark_x87_rs (MIT), verified by its author on an Attack Shark X87 with this same receiver. Untested on this keyboard: Volt reads its settings, changes only the byte of the setting and checks it was stored.",
            )
            .into(),
            notes,
        })
    }

    fn set_control(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
        control: &str,
        value: &Value,
    ) -> Result<(), ProviderError> {
        let dev = self.open(ctx, device)?;
        let mut led = read_valid_led(&dev)?;
        let unknown_effect = || ProviderError::Unsupported(tr("efecto activo desconocido", "unknown active effect").into());
        match control {
            "light.effect" => {
                let id = value
                    .as_int()
                    .filter(|n| (1..=18).contains(n) || *n == cfg::EFFECT_CUSTOM as i64)
                    .ok_or_else(|| ProviderError::Protocol("efecto fuera de rango".into()))?;
                led.set_effect(id as u8);
            }
            "light.brightness" => {
                if !led.set_brightness(level(value, cfg::MAX_LEVEL)?) {
                    return Err(unknown_effect());
                }
            }
            "light.speed" => {
                if !led.set_speed(level(value, cfg::MAX_LEVEL)?) {
                    return Err(unknown_effect());
                }
            }
            "light.palette" => {
                if !led.set_color(level(value, cfg::COLOR_RANDOM)?) {
                    return Err(unknown_effect());
                }
            }
            "power.sleep" => {
                let secs = value
                    .as_int()
                    .filter(|n| (0..=255 * 30).contains(n))
                    .ok_or_else(|| ProviderError::Protocol("tiempo fuera de rango".into()))?;
                led.set_sleep_secs(secs as u32);
            }
            "light.color" => {
                let (r, g, b) = value
                    .as_rgb()
                    .ok_or_else(|| ProviderError::Protocol("color no válido".into()))?;
                let slot = led.slot().ok_or_else(unknown_effect)?;
                let index = led.color().unwrap_or(0).min(cfg::PALETTE_COLORS - 1);
                let mut table = match self.tables.get(&device.id) {
                    Some(t) => t.clone(),
                    None => cfg::read_rgb_table(&dev).map_err(cfg_err)?,
                };
                let o = cfg::palette_offset(slot, index);
                table[o..o + 3].copy_from_slice(&[r, g, b]);
                // Si la escritura falla no se sabe qué tiene el teclado: releer la próxima vez.
                self.tables.remove(&device.id);
                cfg::write_rgb_table(&dev, &table).map_err(cfg_err)?;
                self.tables.insert(device.id.clone(), table);
                // Con «multicolor» el color no se vería: pasar al que se acaba de escribir.
                if led.color() == Some(index) {
                    return Ok(());
                }
                led.set_color(index);
            }
            other => return Err(ProviderError::Unsupported(other.into())),
        }
        cfg::write_led(&dev, &led).map_err(cfg_err)
    }

    /// Las respuestas del receptor compartido a veces se pierden: un solo
    /// "sin respuesta" no basta para dar el teclado por apagado.
    fn offline_is_reliable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_checksum() {
        let r = request();
        assert_eq!((r[0], r[1], r[19]), (0x13, 0x4A, 0x5D));
    }

    #[test]
    fn parses_published_frames() {
        let mut on_battery = [0u8; 20];
        on_battery[..7].copy_from_slice(&[0x13, 0x4A, 0x01, 0x00, 0x02, 0x61, 0x01]);
        on_battery[19] = 0xC2;
        assert_eq!(parse(&on_battery), Some((97, 0x01)));

        let mut cable = [0u8; 20];
        cable[..7].copy_from_slice(&[0x13, 0x4A, 0x01, 0x00, 0x02, 0x64, 0x10]);
        cable[19] = 0xD4;
        assert_eq!(parse(&cable), Some((100, STATE_CABLE)));

        cable[19] = 0x00; // suma incorrecta
        assert_eq!(parse(&cable), None);

        // Variante B (MCHOSE G75 Pro, mismo receptor): 95 %.
        assert_eq!(parse(&[0x13, 0x0A, 0x01, 0x00, 0x04, 0x05, 0x5F, 0x01]), Some((95, 0x01)));
        // Otra trama 0x0A del receptor (no de batería) no debe leerse como batería.
        assert_eq!(parse(&[0x13, 0x0A, 0x01, 0x00, 0x02, 0x01, 0x30, 0x00]), None);
    }

    #[test]
    fn identity_of_redragon_ucal_pro() {
        // Respuesta real a 0x05 del Redragon UCAL Pro en 3554:FA09.
        let r = [
            0x13, 0x05, 0x01, 0x00, 0x0a, 0x03, 0x00, 0x00, 0x00, 0x00, 0xba, 0x00, 0x00, 0x10,
            0x00, 0x00, 0x00, 0x00, 0x00, 0xf0,
        ];
        assert_eq!(parse_identity(&r).as_deref(), Some("0300000000ba"));
        assert_eq!(crate::names::lookup("compx:0300000000ba").map(|e| e.name), Some("REDRAGON UCAL PRO"));
    }
}
