//! Configuración de teclados tras un receptor Compx 2.4G (`3554:FA09`):
//! iluminación, paletas de color y tiempo de reposo.
//!
//! Todo lo de este archivo sale de `attack_shark_x87_rs` (MIT, © 2026 The DT,
//! <https://github.com/thedtvn/attack_shark_x87_rs>), que documenta el protocolo
//! tras desensamblar la herramienta del fabricante (`OemDrv.exe`, clases
//! `CDev3632` para el receptor y `CDevG5KB` para el teclado) y lo verifica en un
//! Attack Shark X87 con este mismo receptor.
//!
//! **Qué está verificado y qué no.** El transporte (tramas de 20 bytes, informe
//! `0x13`) es el mismo que Volt ya usa para la batería del Redragon UCAL Pro: su
//! respuesta de identificación `13 05 01 00 0a 03 00 00 00 00 ba…` encaja byte a
//! byte con este formato (total 1, índice 0, longitud 10). Lo que **no** se ha
//! comprobado en un UCAL Pro es que el bloque de configuración de 128 bytes tenga
//! los mismos desplazamientos que en el X87. Por eso:
//!
//! - nunca se construye un bloque desde cero: se lee el del teclado, se cambian
//!   uno o dos bytes y se devuelve el resto tal cual;
//! - antes de escribir se exige la marca `5A A5` en `0x7E` y `FF FF` en `0x38`
//!   (lo que el driver del fabricante escribe siempre); sin ellas no se toca nada;
//! - tras escribir se vuelve a leer y se comprueba que el cambio está.
//!
//! ```text
//! [0]  0x13
//! [1]  comando             (0x04 escribir config · 0x44 leerla · 0x09/0x49 paletas)
//! [2]  total de paquetes   (respuesta: bit 7 = error de suma)
//! [3]  índice de paquete
//! [4]  sub << 4 | longitud (≤ 14)
//! [5..19] datos
//! [19] suma de los 19 bytes anteriores
//! ```
//!
//! Escritura: paquetes de 14 bytes separados 15 ms, cada uno confirmado por un
//! informe con el mismo comando e índice. Lectura: una petición vacía y el receptor
//! manda los paquetes seguidos.

use hidapi::HidDevice;
use std::thread::sleep;
use std::time::{Duration, Instant};

const REPORT_ID: u8 = 0x13;
const PACKET_LEN: usize = 20;
const DATA_LEN: usize = 14;

pub const CMD_SET_LED: u8 = 0x04;
pub const CMD_GET_LED: u8 = 0x44;
pub const CMD_SET_RGB_TABLE: u8 = 0x09;
pub const CMD_GET_RGB_TABLE: u8 = 0x49;
/// Estado del enlace: `[5]` = 1 si el teclado está enlazado con el receptor.
const CMD_GET_LINK: u8 = 0x07;

pub const LED_LEN: usize = 0x80;
const OFF_PERKEY_ACTIVE: usize = 0x09;
const OFF_EFFECT: usize = 0x0A;
const OFF_SLEEP: usize = 0x18;
const OFF_FIXED: usize = 0x38;
const OFF_SLOTS: usize = 0x3A;
const OFF_MARKER: usize = 0x7E;
const MARKER: [u8; 2] = [0x5A, 0xA5];

/// Nivel máximo de brillo y de velocidad.
pub const MAX_LEVEL: u8 = 4;
/// Índice de color que significa «aleatorio / multicolor».
pub const COLOR_RANDOM: u8 = 7;
pub const PALETTE_COLORS: u8 = 7;
/// Efecto «personalizado» (colores por tecla). Volt no edita ese mapa.
pub const EFFECT_CUSTOM: u8 = 21;

/// Tamaño que se escribe (como el driver con `CRC=1`: relleno hasta 512 con la
/// marca en `0x1FA`) y lo que como mínimo debe devolver una lectura.
pub const RGB_TABLE_LEN: usize = 0x200;
pub const RGB_TABLE_DATA: usize = 0x1A4;
const RGB_TABLE_MARKER: usize = 0x1FA;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfgError {
    Io(String),
    /// No llegó respuesta (teclado dormido, apagado o fuera de alcance).
    Timeout,
    /// El receptor dice que el teclado no está enlazado.
    Asleep,
    /// Respuesta que no encaja con el protocolo.
    Bad(&'static str),
}

fn checksum(p: &[u8]) -> u8 {
    p[..PACKET_LEN - 1].iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

pub fn build_packet(cmd: u8, total: u8, index: u8, sub: u8, data: &[u8]) -> [u8; PACKET_LEN] {
    let n = data.len().min(DATA_LEN);
    let mut p = [0u8; PACKET_LEN];
    p[0] = REPORT_ID;
    p[1] = cmd;
    p[2] = total;
    p[3] = index;
    p[4] = (sub << 4) | n as u8;
    p[5..5 + n].copy_from_slice(&data[..n]);
    p[PACKET_LEN - 1] = checksum(&p);
    p
}

fn io(e: hidapi::HidError) -> CfgError {
    CfgError::Io(e.to_string())
}

fn drain(dev: &HidDevice) {
    let mut buf = [0u8; 64];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
}

/// Junta los paquetes de una lectura. Los repetidos idénticos se aceptan (el
/// receptor reenvía tras una pérdida de radio); los contradictorios, no.
struct Assembler {
    cmd: u8,
    sub: u8,
    len: usize,
    min: usize,
    total: Option<usize>,
    parts: Vec<Option<Vec<u8>>>,
}

impl Assembler {
    fn new(cmd: u8, sub: u8, len: usize, min: usize) -> Self {
        Self {
            cmd,
            sub,
            len,
            min,
            total: None,
            parts: vec![None; len.div_ceil(DATA_LEN)],
        }
    }

    /// `Ok(Some(bloque))` cuando están todos los paquetes; el bloque se rellena
    /// con ceros hasta `len`.
    fn push(&mut self, r: &[u8]) -> Result<Option<Vec<u8>>, CfgError> {
        if r.len() < PACKET_LEN {
            return Ok(None);
        }
        if r[0] == 0xFF {
            return Err(CfgError::Bad("el receptor rechazó la petición"));
        }
        if r[0] != REPORT_ID || r[1] != self.cmd {
            return Ok(None); // batería, enlace u otra respuesta: no es para esta lectura
        }
        if r[2] & 0x80 != 0 {
            return Err(CfgError::Bad("error de suma"));
        }
        if r[4] >> 4 != self.sub {
            return Ok(None);
        }
        let (total, index, n) = (r[2] as usize, r[3] as usize, (r[4] & 0x0F) as usize);
        if total == 0
            || total > self.parts.len()
            || index >= total
            || n == 0
            || n > DATA_LEN
            || (index + 1 < total && n != DATA_LEN)
            || self.total.is_some_and(|t| t != total)
        {
            return Err(CfgError::Bad("paquete fuera de rango"));
        }
        self.total = Some(total);
        let data = r[5..5 + n].to_vec();
        if self.parts[index].as_ref().is_some_and(|old| old != &data) {
            return Err(CfgError::Bad("paquetes contradictorios"));
        }
        self.parts[index] = Some(data);
        if self.parts[..total].iter().any(Option::is_none) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(self.len);
        for part in self.parts[..total].iter().flatten() {
            out.extend_from_slice(part);
        }
        if out.len() < self.min {
            return Err(CfgError::Bad("bloque más corto de lo esperado"));
        }
        out.resize(self.len, 0);
        Ok(Some(out))
    }
}

/// ¿Está el teclado enlazado? `None` si el receptor no contesta a la pregunta.
pub fn linked(dev: &HidDevice) -> Option<bool> {
    drain(dev);
    dev.write(&build_packet(CMD_GET_LINK, 1, 0, 0, &[])).ok()?;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut buf = [0u8; 64];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match dev.read_timeout(&mut buf, left.as_millis().max(1) as i32) {
            Ok(n) if n >= PACKET_LEN && buf[0] == REPORT_ID && buf[1] == CMD_GET_LINK => {
                return Some(buf[5] != 0);
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Si algo falló por tiempo o por una respuesta rota, lo más probable es que el
/// teclado se haya dormido: se pregunta al receptor para decirlo con propiedad.
fn or_asleep<T>(dev: &HidDevice, r: Result<T, CfgError>) -> Result<T, CfgError> {
    match r {
        Err(CfgError::Timeout | CfgError::Bad(_)) if linked(dev) == Some(false) => Err(CfgError::Asleep),
        r => r,
    }
}

/// Lee un bloque de `len` bytes (al menos `min` reales).
pub fn read_block(dev: &HidDevice, cmd: u8, sub: u8, len: usize, min: usize) -> Result<Vec<u8>, CfgError> {
    or_asleep(dev, read_inner(dev, cmd, sub, len, min))
}

fn read_inner(dev: &HidDevice, cmd: u8, sub: u8, len: usize, min: usize) -> Result<Vec<u8>, CfgError> {
    let mut last = CfgError::Timeout;
    let mut buf = [0u8; 64];
    // La primera petición a veces solo despierta el enlace.
    for attempt in 0..4 {
        if attempt > 0 {
            sleep(Duration::from_millis(150));
        }
        drain(dev);
        let mut asm = Assembler::new(cmd, sub, len, min);
        // Petición de lectura: longitud 0 y solo el `sub` en el byte 4.
        let mut req = build_packet(cmd, 1, 0, sub, &[]);
        req[4] = sub << 4;
        req[PACKET_LEN - 1] = checksum(&req);
        log::trace!("compx cfg tx {:02x?}", &req[..5]);
        dev.write(&req).map_err(io)?;

        let mut started = false;
        // Tope de informes por intento: un bloque de 512 bytes son 37 paquetes.
        for _ in 0..256 {
            let wait = if started { 150 } else { 700 };
            let n = dev.read_timeout(&mut buf, wait).map_err(io)?;
            if n == 0 {
                break;
            }
            log::trace!("compx cfg rx {:02x?}", &buf[..n.min(PACKET_LEN)]);
            started |= buf[1] == cmd;
            match asm.push(&buf[..n]) {
                Ok(Some(block)) => return Ok(block),
                Ok(None) => {}
                Err(e) => {
                    last = e;
                    break;
                }
            }
        }
    }
    Err(last)
}

/// Escribe un bloque en paquetes de 14 bytes, esperando la confirmación de cada uno.
pub fn write_block(dev: &HidDevice, cmd: u8, sub: u8, data: &[u8]) -> Result<(), CfgError> {
    or_asleep(dev, write_inner(dev, cmd, sub, data))
}

fn write_inner(dev: &HidDevice, cmd: u8, sub: u8, data: &[u8]) -> Result<(), CfgError> {
    let total = data.len().div_ceil(DATA_LEN);
    if total == 0 || total > 255 {
        return Err(CfgError::Bad("tamaño de bloque no válido"));
    }
    drain(dev);
    for (i, chunk) in data.chunks(DATA_LEN).enumerate() {
        let pkt = build_packet(cmd, total as u8, i as u8, sub, chunk);
        let mut attempt = 0;
        loop {
            sleep(Duration::from_millis(15));
            dev.write(&pkt).map_err(io)?;
            match wait_ack(dev, cmd, i as u8) {
                Ok(()) => break,
                Err(CfgError::Timeout) if attempt < 5 => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

fn wait_ack(dev: &HidDevice, cmd: u8, index: u8) -> Result<(), CfgError> {
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut buf = [0u8; 64];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let n = dev.read_timeout(&mut buf, left.as_millis().max(1) as i32).map_err(io)?;
        if n < 5 {
            continue;
        }
        if buf[0] == 0xFF {
            return Err(CfgError::Bad("el receptor rechazó la escritura"));
        }
        if buf[0] != REPORT_ID || buf[1] != cmd {
            continue; // batería, enlace…
        }
        if buf[2] & 0x80 != 0 {
            return Err(CfgError::Timeout); // error de suma: se reintenta el paquete
        }
        return if buf[3] == index {
            Ok(())
        } else {
            Err(CfgError::Bad("confirmación de otro paquete"))
        };
    }
    Err(CfgError::Timeout)
}

/// El bloque de 128 bytes de iluminación y opciones, tal cual lo da el teclado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedConfig(pub [u8; LED_LEN]);

impl LedConfig {
    pub fn from_bytes(v: &[u8]) -> Option<Self> {
        Some(Self(v.get(..LED_LEN)?.try_into().ok()?))
    }

    /// Las marcas que el driver del fabricante escribe siempre. Si faltan, este
    /// teclado guarda su configuración de otra forma y no se debe escribir.
    pub fn is_valid(&self) -> bool {
        self.0[OFF_MARKER..OFF_MARKER + 2] == MARKER && self.0[OFF_FIXED..OFF_FIXED + 2] == [0xFF, 0xFF]
    }

    pub fn effect(&self) -> u8 {
        self.0[OFF_EFFECT]
    }

    pub fn set_effect(&mut self, id: u8) {
        self.0[OFF_EFFECT] = id;
        self.0[OFF_PERKEY_ACTIVE] = (id == EFFECT_CUSTOM) as u8;
    }

    /// Posición del efecto en las tablas por efecto: los 18 primeros van seguidos
    /// y «personalizado» (21) ocupa la 19.ª. `None` si el id no es conocido.
    pub fn slot(&self) -> Option<usize> {
        match self.effect() {
            id @ 1..=18 => Some(id as usize - 1),
            EFFECT_CUSTOM => Some(18),
            _ => None,
        }
    }

    pub fn brightness(&self) -> Option<u8> {
        self.slot().map(|s| self.0[OFF_SLOTS + 2 * s])
    }

    pub fn speed(&self) -> Option<u8> {
        self.slot().map(|s| self.0[OFF_SLOTS + 2 * s + 1] >> 4)
    }

    /// Índice en la paleta del efecto (0..=6) o `COLOR_RANDOM`.
    pub fn color(&self) -> Option<u8> {
        self.slot().map(|s| self.0[OFF_SLOTS + 2 * s + 1] & 0x0F)
    }

    pub fn set_brightness(&mut self, v: u8) -> bool {
        let Some(s) = self.slot() else { return false };
        self.0[OFF_SLOTS + 2 * s] = v.min(MAX_LEVEL);
        true
    }

    pub fn set_speed(&mut self, v: u8) -> bool {
        let Some(s) = self.slot() else { return false };
        let o = OFF_SLOTS + 2 * s + 1;
        self.0[o] = (v.min(MAX_LEVEL) << 4) | (self.0[o] & 0x0F);
        true
    }

    pub fn set_color(&mut self, index: u8) -> bool {
        let Some(s) = self.slot() else { return false };
        let o = OFF_SLOTS + 2 * s + 1;
        self.0[o] = (self.0[o] & 0xF0) | index.min(COLOR_RANDOM);
        true
    }

    /// Reposo en segundos (el teclado cuenta en unidades de 30 s); 0 = nunca.
    pub fn sleep_secs(&self) -> u32 {
        self.0[OFF_SLEEP] as u32 * 30
    }

    pub fn set_sleep_secs(&mut self, secs: u32) {
        self.0[OFF_SLEEP] = if secs == 0 { 0 } else { secs.div_ceil(30).clamp(1, 255) as u8 };
    }
}

pub fn read_led(dev: &HidDevice) -> Result<LedConfig, CfgError> {
    let v = read_block(dev, CMD_GET_LED, 0, LED_LEN, LED_LEN)?;
    LedConfig::from_bytes(&v).ok_or(CfgError::Bad("bloque de configuración incompleto"))
}

/// Escribe el bloque y comprueba, releyéndolo, que el teclado lo ha guardado.
pub fn write_led(dev: &HidDevice, cfg: &LedConfig) -> Result<(), CfgError> {
    if !cfg.is_valid() {
        return Err(CfgError::Bad("bloque de configuración no reconocido"));
    }
    write_block(dev, CMD_SET_LED, 0, &cfg.0)?;
    sleep(Duration::from_millis(60));
    if read_led(dev)? != *cfg {
        return Err(CfgError::Bad("el teclado no guardó el cambio"));
    }
    Ok(())
}

/// Posición del color `index` de la paleta del efecto en la tabla: 20 filas de 7
/// colores RGB; la fila 0 no se usa y la `slot + 1` es la del efecto.
pub fn palette_offset(slot: usize, index: u8) -> usize {
    (slot + 1) * 21 + index as usize * 3
}

pub fn read_rgb_table(dev: &HidDevice) -> Result<Vec<u8>, CfgError> {
    read_block(dev, CMD_GET_RGB_TABLE, 0, RGB_TABLE_LEN, RGB_TABLE_DATA)
}

/// Escribe la tabla de paletas (con la marca del driver) y comprueba que los
/// datos útiles han quedado como se mandaron.
pub fn write_rgb_table(dev: &HidDevice, table: &[u8]) -> Result<(), CfgError> {
    let mut t = table.to_vec();
    t.resize(RGB_TABLE_LEN, 0);
    t[RGB_TABLE_MARKER..RGB_TABLE_MARKER + 2].copy_from_slice(&MARKER);
    write_block(dev, CMD_SET_RGB_TABLE, 0, &t)?;
    sleep(Duration::from_millis(60));
    let back = read_rgb_table(dev)?;
    if back[..RGB_TABLE_DATA] != t[..RGB_TABLE_DATA] {
        return Err(CfgError::Bad("el teclado no guardó la paleta"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(cmd: u8, sub: u8, total: u8, index: u8, data: &[u8]) -> [u8; PACKET_LEN] {
        build_packet(cmd, total, index, sub, data)
    }

    #[test]
    fn packet_layout_matches_reference() {
        // Mismo vector que el test de attack_shark_x87_rs.
        let p = build_packet(0x04, 10, 0, 0, &[1, 2, 3]);
        assert_eq!(&p[..8], &[0x13, 0x04, 10, 0, 3, 1, 2, 3]);
        assert_eq!(p[19], 0x13 + 4 + 10 + 3 + 1 + 2 + 3);
    }

    #[test]
    fn identity_reply_of_the_ucal_pro_fits_this_framing() {
        // Respuesta real del Redragon UCAL Pro a 0x05 (ver compx.rs): cmd 05,
        // total 1, índice 0, longitud 10 y suma correcta. Es la prueba de que su
        // receptor habla este transporte.
        let r = [
            0x13, 0x05, 0x01, 0x00, 0x0a, 0x03, 0x00, 0x00, 0x00, 0x00, 0xba, 0x00, 0x00, 0x10,
            0x00, 0x00, 0x00, 0x00, 0x00, 0xf0,
        ];
        assert_eq!(checksum(&r), r[19]);
        let mut a = Assembler::new(0x05, 0, 14, 10);
        let block = a.push(&r).unwrap().unwrap();
        assert_eq!(&block[..6], &[0x03, 0x00, 0x00, 0x00, 0x00, 0xba]);
    }

    #[test]
    fn assembles_a_block_out_of_order_and_ignores_other_reports() {
        let data: Vec<u8> = (0..128u8).collect();
        let chunks: Vec<&[u8]> = data.chunks(14).collect();
        let mut a = Assembler::new(CMD_GET_LED, 0, LED_LEN, LED_LEN);
        // Una trama de batería en medio no molesta.
        assert_eq!(a.push(&packet(0x0A, 0, 1, 0, &[5, 90, 1])), Ok(None));
        for i in (1..chunks.len()).rev() {
            assert_eq!(a.push(&packet(CMD_GET_LED, 0, 10, i as u8, chunks[i])), Ok(None));
            // Repetido idéntico: se acepta.
            assert_eq!(a.push(&packet(CMD_GET_LED, 0, 10, i as u8, chunks[i])), Ok(None));
        }
        let out = a.push(&packet(CMD_GET_LED, 0, 10, 0, chunks[0])).unwrap().unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn rejects_short_conflicting_and_flagged_streams() {
        let mut a = Assembler::new(CMD_GET_LED, 0, LED_LEN, LED_LEN);
        // Un único paquete que dice ser el bloque entero: demasiado corto.
        assert!(a.push(&packet(CMD_GET_LED, 0, 1, 0, &[0; 14])).is_err());

        let mut a = Assembler::new(CMD_GET_LED, 0, LED_LEN, LED_LEN);
        a.push(&packet(CMD_GET_LED, 0, 10, 0, &[1; 14])).unwrap();
        assert!(a.push(&packet(CMD_GET_LED, 0, 10, 0, &[2; 14])).is_err());

        let mut a = Assembler::new(CMD_GET_LED, 0, LED_LEN, LED_LEN);
        let mut bad = packet(CMD_GET_LED, 0, 10, 0, &[1; 14]);
        bad[2] |= 0x80;
        assert!(a.push(&bad).is_err());
        let mut nak = packet(CMD_GET_LED, 0, 10, 0, &[1; 14]);
        nak[0] = 0xFF;
        assert!(a.push(&nak).is_err());
    }

    #[test]
    fn rgb_table_read_may_be_shorter_than_what_is_written() {
        // El receptor devuelve 490 bytes (35 paquetes) de los 512 que se escriben.
        let mut a = Assembler::new(CMD_GET_RGB_TABLE, 0, RGB_TABLE_LEN, RGB_TABLE_DATA);
        let mut out = None;
        for i in 0..35u8 {
            out = a.push(&packet(CMD_GET_RGB_TABLE, 0, 35, i, &[i; 14])).unwrap();
        }
        let out = out.unwrap();
        assert_eq!(out.len(), RGB_TABLE_LEN);
        assert_eq!(out[489], 34);
        assert!(out[490..].iter().all(|&b| b == 0));
    }

    fn sample() -> LedConfig {
        let mut b = [0u8; LED_LEN];
        b[OFF_EFFECT] = 2; // respiración
        b[OFF_SLEEP] = 10; // 5 min
        b[OFF_FIXED] = 0xFF;
        b[OFF_FIXED + 1] = 0xFF;
        b[OFF_SLOTS + 2] = 3; // brillo del efecto 2
        b[OFF_SLOTS + 3] = 0x21; // velocidad 2, color 1
        b[OFF_MARKER] = 0x5A;
        b[OFF_MARKER + 1] = 0xA5;
        LedConfig(b)
    }

    #[test]
    fn led_config_fields() {
        let mut c = sample();
        assert!(c.is_valid());
        assert_eq!((c.effect(), c.brightness(), c.speed(), c.color()), (2, Some(3), Some(2), Some(1)));
        assert_eq!(c.sleep_secs(), 300);

        // Cada cambio toca solo su byte (o su medio byte).
        let before = c.clone();
        assert!(c.set_speed(4));
        assert_eq!(c.0[OFF_SLOTS + 3], 0x41);
        assert!(c.set_color(COLOR_RANDOM));
        assert_eq!(c.0[OFF_SLOTS + 3], 0x47);
        assert!(c.set_brightness(9));
        assert_eq!(c.brightness(), Some(MAX_LEVEL));
        c.set_sleep_secs(45);
        assert_eq!(c.sleep_secs(), 60);
        c.set_sleep_secs(0);
        assert_eq!(c.sleep_secs(), 0);
        let changed: Vec<usize> = (0..LED_LEN).filter(|&i| c.0[i] != before.0[i]).collect();
        assert_eq!(changed, vec![OFF_SLEEP, OFF_SLOTS + 2, OFF_SLOTS + 3]);

        c.set_effect(EFFECT_CUSTOM);
        assert_eq!((c.0[OFF_PERKEY_ACTIVE], c.slot()), (1, Some(18)));
        c.set_effect(1);
        assert_eq!((c.0[OFF_PERKEY_ACTIVE], c.slot()), (0, Some(0)));
    }

    #[test]
    fn unknown_layouts_are_refused() {
        let mut c = sample();
        c.0[OFF_MARKER] = 0;
        assert!(!c.is_valid());
        let mut c = sample();
        c.0[OFF_FIXED] = 0;
        assert!(!c.is_valid());
        // Efecto que no está en la tabla conocida: no hay brillo que tocar.
        let mut c = sample();
        c.0[OFF_EFFECT] = 40;
        assert_eq!(c.slot(), None);
        assert!(!c.set_brightness(1));
        assert_eq!(c, {
            let mut s = sample();
            s.0[OFF_EFFECT] = 40;
            s
        });
    }

    #[test]
    fn palette_offsets() {
        assert_eq!(palette_offset(0, 0), 21); // la fila 0 no se usa
        assert_eq!(palette_offset(1, 6), 42 + 18);
        assert!(palette_offset(18, 6) + 3 <= RGB_TABLE_DATA);
    }
}
