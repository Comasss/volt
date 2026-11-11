//! Diagnóstico: lista todas las interfaces HID visibles con su VID/PID.
//!
//!   cargo run --bin hid-list            # todo
//!   cargo run --bin hid-list -- 046d    # filtrar por VID (hex)
//!   cargo run --bin hid-list -- --paths # incluir rutas completas
//!   cargo run --bin hid-list -- --probe # ejecutar los providers reales una vez
//!   cargo run --bin hid-list -- --probe --controls # + ajustes configurables (solo lectura)
//!
//! Sin `--probe` no abre ningún dispositivo: solo enumera, así que es seguro con
//! G HUB/Synapse abiertos. `--probe` sí habla con el hardware (solo lecturas).

use hidapi::HidApi;
use std::time::Instant;
use volt_lib::device_ids::{lookup, vendor_name};
use volt_lib::hid::HidContext;
use volt_lib::provider::PollContext;
use volt_lib::providers;

fn probe(keep_eq: bool, controls: bool) {
    let hid = HidContext::new()
        .map_err(|e| eprintln!("hidapi: {e}"))
        .ok();
    // Por defecto solo lee; `--keep-eq` aplica el ajuste de EQ del BlackShark como la app.
    let ctx = PollContext {
        hid: hid.as_ref(),
        razer_keep_eq: keep_eq,
        forced: true,
    };
    for mut p in providers::all() {
        let t = Instant::now();
        println!("[{}]", p.id());
        match p.discover(&ctx) {
            Ok(list) if list.is_empty() => println!("  (sin dispositivos)"),
            Ok(list) => {
                for d in list {
                    let t1 = Instant::now();
                    let r = p.battery(&ctx, &d);
                    println!(
                        "  {:<28} {:<10} {:<22} {:?}",
                        d.name,
                        format!("{:?}", d.kind),
                        d.via.as_deref().unwrap_or("-"),
                        d.connection
                    );
                    match r {
                        Ok(b) => println!(
                            "      batería {:?}%{} carga {:?}{}  ({:?})",
                            b.percent,
                            if b.approximate { " (aprox.)" } else { "" },
                            b.charge,
                            b.voltage_mv.map(|v| format!(" {v} mV")).unwrap_or_default(),
                            t1.elapsed()
                        ),
                        Err(e) => println!("      {e}  ({:?})", t1.elapsed()),
                    }
                    // `--controls`: lo que se podría personalizar y su valor actual.
                    // Solo lee; sirve para comprobar un protocolo antes de escribir nada.
                    if controls && p.capabilities(&d).configurable() {
                        let t2 = Instant::now();
                        match p.controls(&ctx, &d) {
                            Ok(c) => {
                                for ctl in &c.controls {
                                    println!(
                                        "      · {:<22} = {:<28} {:?}{}",
                                        ctl.id,
                                        ctl.value.as_ref().map(|v| format!("{v:?}")).unwrap_or_else(|| "?".into()),
                                        ctl.trust,
                                        if ctl.volatile { " (volátil)" } else { "" }
                                    );
                                }
                                for n in &c.notes {
                                    println!("      ! {n}");
                                }
                                println!("      ajustes leídos en {:?}", t2.elapsed());
                            }
                            Err(e) => println!("      ajustes: {e}  ({:?})", t2.elapsed()),
                        }
                    }
                }
            }
            Err(e) => println!("  error: {e}"),
        }
        p.release();
        println!("  total {:?}\n", t.elapsed());
    }
}

fn parse_vid_pid(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
}

/// Resume un descriptor de informe: tamaño de cada informe por tipo e ID.
fn summarize_descriptor(d: &[u8]) -> Vec<String> {
    use std::collections::BTreeMap;
    let (mut size, mut count, mut id, mut page) = (0u32, 0u32, 0u8, 0u32);
    let mut bits: BTreeMap<(&str, u8), (u32, u32)> = BTreeMap::new();
    let mut i = 0;
    while i < d.len() {
        let b = d[i];
        let n = match b & 3 {
            3 => 4,
            x => x as usize,
        };
        let mut v = 0u32;
        for k in 0..n {
            v |= (*d.get(i + 1 + k).unwrap_or(&0) as u32) << (8 * k);
        }
        match (b >> 2 & 3, b >> 4) {
            (1, 0x0) => page = v,
            (1, 0x7) => size = v,
            (1, 0x8) => id = v as u8,
            (1, 0x9) => count = v,
            (0, t @ (0x8 | 0x9 | 0xB)) => {
                let kind = match t {
                    0x8 => "INPUT",
                    0x9 => "OUTPUT",
                    _ => "FEATURE",
                };
                let e = bits.entry((kind, id)).or_insert((0, page));
                e.0 += size * count;
            }
            _ => {}
        }
        i += 1 + n;
    }
    bits.into_iter()
        .map(|((k, id), (b, page))| {
            format!("{k:<7} id 0x{id:02X}  {:>3} bytes de datos  (usage page {page:04X})", b.div_ceil(8))
        })
        .collect()
}

/// `--desc VID:PID`: descriptores de informe de cada colección (no escribe nada).
fn describe(api: &HidApi, vid: u16, pid: u16) {
    for d in api.device_list().filter(|d| d.vendor_id() == vid && d.product_id() == pid) {
        println!(
            "\nIF {}  usage {:04X}:{:04X}  {}",
            d.interface_number(),
            d.usage_page(),
            d.usage(),
            d.path().to_string_lossy()
        );
        match d.open_device(api) {
            Ok(dev) => {
                let mut buf = [0u8; 4096];
                match dev.get_report_descriptor(&mut buf) {
                    Ok(n) => {
                        for line in summarize_descriptor(&buf[..n]) {
                            println!("  {line}");
                        }
                        println!("  raw: {:02x?}", &buf[..n]);
                    }
                    Err(e) => println!("  (sin descriptor: {e})"),
                }
            }
            Err(e) => println!("  (no se pudo abrir: {e})"),
        }
    }
}

/// `--sniff VID:PID [segundos]`: escucha pasiva de informes de entrada en todas las
/// colecciones que Windows deja leer. No envía nada al dispositivo. Útil para ver las
/// respuestas que provoca la app del fabricante.
fn sniff(api: &HidApi, vid: u16, pid: u16, secs: u64) {
    let mut handles = Vec::new();
    for d in api.device_list().filter(|d| d.vendor_id() == vid && d.product_id() == pid) {
        // Teclado y ratón los tiene el sistema en exclusiva: no se pueden leer.
        if d.usage_page() == 1 && matches!(d.usage(), 2 | 6) {
            continue;
        }
        if let Ok(dev) = d.open_device(api) {
            handles.push((format!("IF{} {:04X}:{:04X}", d.interface_number(), d.usage_page(), d.usage()), dev));
        }
    }
    println!("escuchando {} colecciones durante {secs} s...", handles.len());
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    while start.elapsed().as_secs() < secs {
        for (name, dev) in &handles {
            if let Ok(n) = dev.read_timeout(&mut buf, 20) {
                if n > 0 {
                    println!("{:>8.3}s  {name}  {:02x?}", start.elapsed().as_secs_f32(), &buf[..n]);
                }
            }
        }
    }
}

/// `--compx-scan`: identifica qué contesta un receptor Compx compartido para cada
/// índice de dispositivo (consultas 0x05 y 0x4A, de solo lectura).
fn compx_scan(api: &HidApi) {
    let Some(info) = api.device_list().find(|d| {
        d.vendor_id() == 0x3554 && d.usage_page() == 0xFF02 && d.usage() == 0x0002
    }) else {
        println!("No hay receptor Compx con colección FF02:0002.");
        return;
    };
    println!("receptor {:04X}:{:04X}", info.vendor_id(), info.product_id());
    let dev = info.open_device(api).expect("abrir");
    let frame = |cmd: u8, arg: u8| {
        let mut p = [0u8; 20];
        p[0] = 0x13;
        p[1] = cmd;
        p[2] = arg;
        p[19] = p[..19].iter().fold(0u8, |a, x| a.wrapping_add(*x));
        p
    };
    let mut buf = [0u8; 64];
    for (cmd, arg) in [(0x05, 0), (0x05, 1), (0x05, 2), (0x05, 3), (0x4A, 1), (0x4A, 2)] {
        while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
        println!("\ntx {cmd:02X} {arg:02X}");
        let _ = dev.write(&frame(cmd, arg));
        let start = Instant::now();
        while start.elapsed().as_millis() < 1200 {
            if let Ok(n) = dev.read_timeout(&mut buf, 100) {
                if n > 0 {
                    println!("  {:>5} ms  {:02x?}", start.elapsed().as_millis(), &buf[..n]);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--compx-scan") {
        compx_scan(&HidApi::new().expect("hidapi"));
        return;
    }
    let flag_value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|s| parse_vid_pid(s))
    };
    // Estado de audio del BlackShark V2 Pro 2023 (solo lecturas).
    if args.iter().any(|a| a == "--blackshark") {
        let api = HidApi::new().expect("hidapi");
        let Some(info) = api.device_list().find(|d| {
            d.vendor_id() == 0x1532
                && volt_lib::providers::razer_mxic::PIDS.contains(&d.product_id())
                && d.usage_page() == volt_lib::providers::razer_mxic::USAGE_PAGE
        }) else {
            println!("No hay ningún BlackShark V2 Pro 2023 conectado.");
            return;
        };
        let dev = info.open_device(&api).expect("abrir");
        // --blackshark --eq-status 0|1 : escribe el registro 0x9E (lo que hace Synapse al arrancar).
        if let Some(v) = args
            .iter()
            .position(|a| a == "--eq-status")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<u8>().ok())
        {
            use volt_lib::providers::razer_mxic as m;
            match m::write_value(&dev, m::CMD_PRESET_EQ_STATUS, v) {
                Ok(back) => println!("0x9E ← {v}; el auricular ahora dice {back:?}"),
                Err(e) => println!("error: {e:?}"),
            }
        }
        match volt_lib::providers::razer_mxic::read_audio_state(&dev) {
            Ok(s) => println!("{s}"),
            Err(e) => println!("error: {e:?}"),
        }
        return;
    }
    if let Some((vid, pid)) = flag_value("--desc") {
        describe(&HidApi::new().expect("hidapi"), vid, pid);
        return;
    }
    if let Some((vid, pid)) = flag_value("--sniff") {
        let secs = args
            .iter()
            .position(|a| a == "--sniff")
            .and_then(|i| args.get(i + 2))
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        sniff(&HidApi::new().expect("hidapi"), vid, pid, secs);
        return;
    }
    if args.iter().any(|a| a == "--probe") {
        // RUST_LOG=volt_lib=trace muestra los informes HID++ en crudo.
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("volt_lib=info"))
            .init();
        probe(args.iter().any(|a| a == "--keep-eq"), args.iter().any(|a| a == "--controls"));
        return;
    }
    let show_paths = args.iter().any(|a| a == "--paths");
    let vid_filter = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok());

    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("No se pudo inicializar hidapi: {e}");
            std::process::exit(1);
        }
    };

    let mut rows: Vec<_> = api
        .device_list()
        .filter(|d| vid_filter.is_none_or(|v| d.vendor_id() == v))
        .collect();
    rows.sort_by_key(|d| {
        (
            d.vendor_id(),
            d.product_id(),
            d.interface_number(),
            d.usage_page(),
            d.usage(),
        )
    });

    println!(
        "{:<9}  {:<20}  {:>2}  {:<11}  {:<28}  {}",
        "VID:PID", "FABRICANTE", "IF", "USAGE", "PRODUCTO", "NOTA"
    );
    println!("{}", "-".repeat(100));
    for d in &rows {
        let (vid, pid) = (d.vendor_id(), d.product_id());
        let vendor = vendor_name(vid).unwrap_or("?");
        let known = lookup(vid, pid).map(|k| k.name).unwrap_or("");
        let note = match (vid, d.usage_page(), d.usage()) {
            (0x046D, 0xFF00, 0x0001) => "HID++ corto".to_string(),
            (0x046D, 0xFF00, 0x0002) => "HID++ largo".to_string(),
            (0x046D, 0xFF43, _) => "HID++ BLE".to_string(),
            (_, up, _) if up >= 0xFF00 => "vendor".to_string(),
            _ => String::new(),
        };
        let note = [known, note.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        let product: String = d.product_string().unwrap_or("").chars().take(28).collect();
        println!(
            "{vid:04X}:{pid:04X}  {vendor:<20.20}  {:>2}  {:04X}:{:04X}  {product:<28}  {note}",
            d.interface_number(),
            d.usage_page(),
            d.usage(),
        );
        if show_paths {
            println!("           {}", d.path().to_string_lossy());
        }
    }
    println!("\n{} interfaces HID", rows.len());
}
