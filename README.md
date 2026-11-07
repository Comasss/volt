# VOLT · monitor de batería de periféricos

App de bandeja para Windows 10/11 (Rust + Tauri 2) que lee la batería de ratones,
teclados y auriculares inalámbricos sin las apps de los fabricantes.

**Compatibles hoy:**
- Logitech HID++ (LIGHTSPEED, Unifying, Bolt, cable).
- Razer BlackShark V2 Pro 2023.
- Attack Shark (receptores `1D57`).
- Teclados con receptor Compx (Redragon, AULA, MCHOSE).
- Cualquier dispositivo Bluetooth cuya batería conozca Windows.

## Descargar

En **[Releases](../../releases/latest)** está el instalador `Volt_…_x64-setup.exe`.

- No está firmado, así que Windows SmartScreen puede avisar: *Más información →
  Ejecutar de todas formas*.
- WebView2 viene con Windows 11. En Windows 10, el instalador lo descarga si falta.
- Junto al instalador se publica `hid-list.exe`, la herramienta de diagnóstico.

## Publicar una versión

GitHub Actions lo hace todo:
- `.github/workflows/ci.yml` compila y pasa los tests en cada push.
- `.github/workflows/release.yml` compila el instalador y crea la *release* al subir una
  etiqueta `vX.Y.Z`.

Pasos:

```powershell
node scripts/bump-version.mjs 0.2.0       # versión en package.json, Cargo.toml, tauri.conf.json y Cargo.lock
git commit -am "Volt 0.2.0"
git tag v0.2.0
git push --follow-tags
```

Si la etiqueta no coincide con la versión de `tauri.conf.json`, el workflow falla
antes de compilar, para no publicar una versión con el número equivocado. También se
puede lanzar a mano desde la pestaña *Actions* (*Release › Run workflow*): publica la
versión que tenga `tauri.conf.json`.

> El nombre *Volt* es provisional. Para cambiarlo, edita:
> - `productName` e `identifier` en `src-tauri/tauri.conf.json`;
> - `name` (y `default-run`) en `src-tauri/Cargo.toml`;
> - `<title>` y `.brand-name` en `index.html`.

## Estructura

```
battery/
├─ index.html                 ventana: barra de título propia + árbol + contenido
├─ src/                       frontend (TypeScript sin framework)
│  ├─ main.ts                 render de fichas, árbol, ajustes, diagnóstico
│  ├─ backend.ts              IPC/eventos Tauri · simulación si se abre en navegador
│  ├─ styles.css              estética de ficha técnica (claro/oscuro)
│  ├─ icons.ts, types.ts
├─ app-icon.svg               fuente de los iconos (npx tauri icon app-icon.svg)
└─ src-tauri/
   ├─ Cargo.toml, tauri.conf.json, capabilities/default.json
   └─ src/
      ├─ lib.rs               arranque, plugins, ciclo de vida
      ├─ provider.rs          trait DeviceProvider + ProviderError + Capabilities
      ├─ manager.rs           hilo de sondeo único, avisos, evento volt://state
      ├─ model.rs             DeviceInfo, BatteryReading, DeviceSnapshot…
      ├─ device_ids.rs        tabla de fabricantes y VID/PID (ampliable)
      ├─ hid.rs               enumeración HID compartida por ciclo
      ├─ providers/
      │  ├─ logitech/         HID++ 2.0 (0x1004, 0x1000, 0x1001, 0x0005)
      │  ├─ razer.rs          ratones/teclados: informe de 90 bytes (experimental)
      │  ├─ razer_mxic.rs     auriculares HyperSpeed "Audio MXIC" (BlackShark V2 Pro 2023)
      │  ├─ compx.rs          receptores Compx 2.4G (Redragon, AULA, MCHOSE…)
      │  ├─ xenta.rs          receptores 1D57 (Attack Shark X6/X7…), escucha pasiva
      │  ├─ bluetooth.rs      propiedad PnP de batería + GATT 0x180F (WinRT)
      │  └─ mock.rs           dispositivos simulados
      ├─ tray.rs              icono dinámico, tooltip y menú
      ├─ window.rs            crear/destruir la ventana, esquinas rectas (DWM)
      ├─ notify.rs, settings.rs, commands.rs
      └─ bin/hid_list.rs      herramienta de diagnóstico
```

## Requisitos

- Rust estable con toolchain **MSVC** (`rustup default stable-x86_64-pc-windows-msvc`)
  y *Visual Studio Build Tools* (carga de trabajo «Desarrollo para el escritorio con C++»).
- Node.js 20.19+ (o 22.12+).
- WebView2: viene con Windows 11; en Windows 10 lo instala el instalador si falta.

## Compilar y ejecutar

```powershell
npm install
npm run tauri dev                 # desarrollo (recarga en caliente del frontend)
npm run tauri dev -- -- --mock    # con dispositivos simulados sin tocar ajustes
npm run tauri build               # instalador NSIS en src-tauri/target/release/bundle/nsis
npm run tauri build -- --no-bundle  # solo el .exe en src-tauri/target/release/volt.exe
```

Logs (en `tauri dev`, en la consola): `$env:RUST_LOG="volt_lib=debug"`, o `=trace`
para ver cada informe HID++ en crudo.

Solo la interfaz, sin Rust: `npm run dev` y abre `http://localhost:1420` en un
navegador (usa datos simulados).

Argumentos del `.exe`: `--hidden` (arrancar solo en la bandeja; lo añade el
autoarranque) y `--mock`.

## Depurar qué dispositivos HID se detectan

```powershell
cd src-tauri
cargo run --bin hid-list                 # todas las colecciones HID: VID:PID, usage page, producto
cargo run --bin hid-list -- 046d         # filtrar por VID
cargo run --bin hid-list -- --paths      # con rutas de dispositivo
cargo run --bin hid-list -- --probe      # ejecuta los providers reales una vez y muestra la lectura
$env:RUST_LOG="volt_lib=trace"; cargo run --bin hid-list -- --probe   # + tráfico en crudo
cargo run --bin hid-list -- --desc 1532:0555        # descriptores de informe (IDs, tamaños) de cada colección
cargo run --bin hid-list -- --sniff 3554:fa09 60    # escucha pasiva de informes de entrada durante 60 s
cargo run --bin hid-list -- --blackshark            # preset/EQ activos del BlackShark V2 Pro (solo lectura)
cargo run --bin hid-list -- --blackshark --eq-status 0   # escribir el registro de EQ (0 = como Synapse)
cargo run --bin hid-list -- --probe --keep-eq       # sondeo aplicando el ajuste de EQ, como la app
```

Para un dispositivo nuevo: `--desc` dice qué informes acepta (feature, output, de qué
tamaño). `--sniff` con la app del fabricante abierta enseña las respuestas que esta
provoca.

Lo mismo está en la app en **HERRAMIENTAS › DIAGNÓSTICO HID** (la columna PROVIDER
indica qué colecciones atiende cada provider). Sin `--probe`, `hid-list` solo enumera:
no abre nada y es seguro con G HUB o Synapse abiertos.

Salida real en este equipo:

```
[logitech]
  PRO X 2              Mouse      LIGHTSPEED C54D     Receiver
      batería Some(92)% carga Discharging  (17ms)
[razer]
  BLACKSHARK V2 PRO    Headset    HYPERSPEED 0555     Receiver
      batería Some(72)% carga Discharging  (182ms)
[compx]
  TECLADO 2.4G         Keyboard   COMPX 2.4G FA09     Receiver
      batería Some(93)% carga Discharging  (1.05s la primera vez; luego recuerda la variante)
```

## Arquitectura

- **`DeviceProvider`** (síncrono): `discover()`, `battery()`, `release()` y, con
  implementación por defecto «no soportado», `set_dpi()`, `set_lighting()` y
  `set_polling_rate()`. Un provider por fabricante o protocolo.
- **Gestor**: un único hilo que duerme en `recv_timeout(intervalo)`. Despierta por
  tiempo o por una orden (escanear, ajustes, diagnóstico). Enumera HID **una vez por
  ciclo** y lo comparte. Cada provider va dentro de `catch_unwind`, así que un fallo
  en uno no afecta al resto. Al final del ciclo se cierran todos los handles.
- **Backend → frontend**: evento `volt://state` tras cada sondeo. El JS no tiene
  temporizadores: al abrir la ventana pide el último estado (`get_state`, sin tocar
  hardware) y después solo escucha.
- **Errores**: `Offline` (dormido o fuera de alcance, se conserva la última lectura),
  `Busy` (otra app), `NoResponse`, `Unsupported`... Se muestran en la ficha como
  «no disponible» con el motivo, sin interrumpir nada más.
- **Avisos**: una notificación al bajar del umbral sin estar cargando. Se rearma al
  cargar o al subir 5 puntos por encima del umbral, para no repetir el aviso.
- **Añadir un fabricante**: ver el comentario de `providers/mod.rs` y la tabla
  `device_ids.rs`.

## Nombres, actualización al momento, idioma y avisos

- **Nombres exactos.** Cada provider calcula una *huella* con lo que el dispositivo
  dice de sí mismo; no basta el VID/PID, porque los receptores genéricos lo
  comparten entre marcas. Ejemplos:
  - Compx: el ID de modelo de la identificación `0x05` (`compx:0300000000ba`).
  - Logitech: el receptor y la ranura.
  - Bluetooth: la dirección.

  El nombre se elige en este orden:
  1. El que pone el usuario (lápiz en la ficha, guardado en `settings.json`).
  2. `src-tauri/devices.json`, la base de datos incluida en el binario.
  3. El que informa el dispositivo.
  4. Uno genérico.

  La ficha indica de dónde sale el nombre.
- **Bases de datos de nombres:**
  - `src-tauri/devices.json`: tabla propia, cada entrada con su fuente. Lleva un nivel
    de confianza: `exact` cuando la huella identifica un solo modelo, y `probable`
    cuando la comparten varios. Por ejemplo, el receptor `1D57:FA60` lo usan el
    Attack Shark X6 y el X7, y la ficha lo marca como «probable».
  - `src-tauri/usb-ids.txt`: subconjunto de [usb.ids](http://www.linux-usb.org/usb.ids)
    (versión 2026.06.26, licencia BSD-3-Clause, © Stephen J. Gowdy y colaboradores).
    Son 1.329 productos de 29 fabricantes de periféricos: Logitech, Razer, SteelSeries,
    Corsair, HyperX, Sony, Microsoft, ASUS ROG, 8BitDo, Turtle Beach… Se usa como
    respaldo por VID:PID y en *Diagnóstico HID*. Para regenerarlo, usa el script
    `usbids_subset.py` descrito en el código.
  - Para añadir un dispositivo: una línea en `devices.json` con su huella, que aparece
    en *Diagnóstico HID* y en el `fingerprint` del estado. O renómbralo desde su ficha.
- **Al momento, sin sondeo:**
  - Windows avisa con `CM_Register_Notification` cuando aparece o desaparece una
    interfaz HID (cable, receptor, Bluetooth), y Volt lee 1,5 s después.
  - Los dispositivos que envían su estado por sí solos (Attack Shark, BlackShark,
    Compx) tienen un hilo bloqueado en la lectura que publica cada cambio de batería,
    carga o enlace.
- **Desconectados:**
  - Lo que deja de aparecer en el sistema se quita de la lista.
  - Lo que avisa de que se ha apagado se oculta al momento: el BlackShark envía el
    evento de enlace `0x20`; el Attack Shark se da por apagado tras 12 s sin su informe.
  - Un dispositivo que no contesta se oculta tras dos sondeos.
  - *Ajustes › Mostrar desconectados* los vuelve a enseñar con su último valor.
- **Idioma:** español o inglés según Windows (`GetUserDefaultUILanguage`), o el que
  se elija en Ajustes. Afecta a la interfaz, la bandeja, las notificaciones y los
  errores.
- **Notificaciones como "Volt":** se registra el AppUserModelID `dev.volt.battery`
  en `HKCU\Software\Classes\AppUserModelId`, con nombre e icono, y los avisos se
  envían por WinRT a ese ID. Funciona sin instalar y sin administrador. Para probarlo:
  el botón *Probar* de Ajustes o `volt.exe --test-notification`.

## Consumo

- Con la ventana cerrada, la ventana y el WebView2 **se destruyen** (no se ocultan).
  Solo queda `volt.exe` en la bandeja.
- En reposo no hay bucles activos: el hilo de sondeo duerme hasta el siguiente ciclo.
  Un ciclo con un receptor LIGHTSPEED tarda unos 0,1–0,3 s cada 60 s.
- Medido en este equipo (Windows 11):

  | Estado | volt.exe (privada) | WebView2 | CPU |
  |---|---|---|---|
  | Ventana abierta (release) | 5,6 MB | ~230 MB en 6 procesos | ~0,3 % de un núcleo de media en 2 min |
  | Ventana cerrada (debug) | 5,5 MB (≈35 MB working set) | 0 procesos | despierta 1 vez/min ~0,3 s |

  La memoria grande es toda de WebView2 y desaparece al cerrar la ventana. Reabrirla
  tarda menos de un segundo y el estado sale de memoria, sin volver a leer el hardware.

## Limitaciones reales

**Logitech (HID++)**
- Funcionan los dispositivos HID++ 2.0 por receptor LIGHTSPEED/Unifying/Bolt, por
  cable USB y por BLE con colección vendor. Los HID++ 1.0 antiguos (anteriores a
  ~2013) salen como «sin soporte».
- Un ratón **dormido al arrancar la app** no aparece hasta que despierta: el receptor
  LIGHTSPEED no contesta por esa ranura. Si ya se había visto, pasa a «desconectado»
  y conserva su último valor.
- Los auriculares G (PRO X 2 LIGHTSPEED, G733, G535…) usan otros receptores y
  features de batería. Todavía no están implementados.
- **G HUB puede estar abierto**: en Windows cada handle recibe todas las respuestas,
  y se distinguen por el *software id* de HID++. No hay bloqueo exclusivo.

**Razer**
- **BlackShark V2 Pro 2023 (`1532:0555`)**: funciona (batería y carga) con el protocolo
  «Audio MXIC». Fuentes: [rzr](https://github.com/Ashesh3/razer-device-control)
  (MIT) y el driver de OpenRazer para este modelo. Cada lectura activa y libera el
  *remote mode*, como hacen Synapse y OpenRazer. Probado aquí con Synapse abierto,
  sin conflictos. El BlackShark V2 Pro de 2020 (`0528`) y otros auriculares Razer usan
  otros protocolos y no están implementados.
- Ratones y teclados: protocolo de 90 bytes de openrazer, con sondeo automático del
  *transaction id*. Es experimental: no se ha probado con hardware. Con Synapse abierto
  puede haber respuestas cruzadas, porque ambos usan el mismo informe de función. Se
  detecta y la ficha muestra «ocupado» durante ese ciclo.

**BlackShark V2 Pro sin Synapse (volumen/EQ)**
- Al cerrar Synapse el sonido baja. Ni el volumen de Windows ni THX cambian (el
  servicio THX `VSSrv.exe` sigue aplicando espacial y EQ). Lo único que cambia es un
  registro del auricular, *Speaker Preset EQ Status* (lectura `0x1E`, escritura
  `0x9E`): Synapse lo pone a 0 al arrancar y al cerrarse vuelve a 1.
- Con **AJUSTES › AUDIO › BLACKSHARK V2 PRO · EQ SIN SYNAPSE** activado (por defecto),
  en cada lectura de batería, si Synapse no está abierto y el registro vale 1, Volt lo
  pone a 0. Es la misma trama que manda Synapse.
- Además, mientras Synapse está abierto, un hilo queda esperando su cierre bloqueado en
  el kernel (`WaitForSingleObject`, sin sondeo). Al cerrarse, Volt reajusta el auricular
  a los ~3 s, sin esperar al siguiente ciclo.
- Si apagas y enciendes el auricular con Synapse cerrado, se corrige en el siguiente
  sondeo (≤ intervalo). Es la única escritura que hace Volt en un dispositivo. Para
  deshacerla a mano: `cargo run --bin hid-list -- --blackshark --eq-status 1`.

**Compx (Redragon, AULA, MCHOSE…)**
- Receptor `3554:FA09`. Hay dos variantes de firmware: comando `0x4A` (AULA), o sesión
  `0x05` y trama `0x0A` (tu Redragon). Se prueban las dos y se recuerda cuál contesta.
- Solo se habla con los PID de la tabla: el mismo VID lo usan ratones VXE/ATK con
  otros comandos. Para añadir otro receptor de teclado, una línea `Protocol::Compx` en
  `device_ids.rs`.
- Con cable conectado el teclado no da porcentaje: sale «cargando» sin cifra.
- **Receptor compartido**: FA09 puede llevar emparejados un teclado y un ratón (aquí el
  Redragon y un Attack Shark X7). La trama de batería no indica de qué dispositivo es,
  y el receptor ignora el índice de 0x05. Con el X7 apagado sigue llegando el mismo
  valor, así que es la del **teclado**. La batería del X7 no sale por este canal. El
  protocolo del X7 no está documentado: si tiene receptor propio, `hid-list` con él
  enchufado es el punto de partida.
- Cada consulta despierta el enlace del teclado, y algunos (como el Redragon UCAL Pro)
  **encienden sus luces** al recibirla. Por eso:
  - se le pregunta como mucho cada 5 minutos; *Escanear* o enchufar/desenchufar algo
    fuerzan la lectura, y entre medias se muestra la última con su hora real;
  - una vez conocida la variante, solo se manda su comando (`0x05` o `0x4A`), nunca el
    que el teclado no entiende.
- Una lectura sin respuesta no cambia el estado: el teclado solo se da por apagado tras
  tres seguidas, para que no parpadee entre «en línea» y «desconectado» al dormitar.
- Por Bluetooth («BT5.0 KB») lo cubre el provider Bluetooth.

**Bluetooth**
- La fuente principal es el valor que el propio Windows guarda en el dispositivo PnP:
  el mismo que ves en Configuración › Bluetooth. Cubre BLE con Battery Service y
  auriculares clásicos que informan por HFP. No hay estado de carga, porque el Battery
  Service básico no lo incluye.
- Los dispositivos desconectados se siguen mostrando 7 días con su último valor y su
  fecha. Los teléfonos y ordenadores se ignoran.
- La lectura GATT directa (WinRT) solo se usa para BLE conectados que tienen 0x180F
  pero sin valor PnP. **No se ha podido probar aquí** porque no había ninguno conectado.
- Mandos (DualSense, Xbox) y altavoces A2DP sin HFP no exponen batería por estas vías.

**Attack Shark (receptores `1D57`, chip "Xenta")**
- El X7 usa su propio receptor `1D57:FA60`. La batería sale del driver oficial del X6,
  analizado sin ejecutarlo. Pasos:
  - extraer el instalador Inno Setup 6.1 y deshacer su filtro de `CALL`/`JMP`;
    las DLL de Microsoft del paquete vuelven a validar firma;
  - desensamblar con `dumpbin` de Visual Studio.
- Resultado: `hiddriver.dll` lee informes de entrada de 5 bytes en la colección
  `MI_02&Col03` (usage page `0x000A`). `03 10 40 <estado> <nivel>` es la batería:
  nivel 1–10 (×10 %), estado 1 batería, 2 lleno, 3 cargando.
- **El ratón lo envía solo cada ~4 s**, así que Volt únicamente escucha hasta 5 s por
  ciclo: no le manda nada. La configuración (DPI, luces, macros) usa feature reports de
  262 bytes en `Col04`, y Volt no la toca.
- Precisión de 10 % (lo que da el ratón). Si está apagado o dormido no llega nada y sale
  como desconectado.
- `1D57:FA60/FA61` son PIDs genéricos del chip: otros ratones con el mismo receptor
  probablemente funcionen, pero saldrán con el nombre de la tabla.

**Otros fabricantes**: SteelSeries, Corsair, HyperX, ratones VXE/ATK… no están
implementados (el hueco está preparado). Tus dispositivos HyperX son micrófonos
QuadCast S, sin batería.

**Permisos de administrador: no hacen falta.** HID se abre en modo usuario (las
colecciones de ratón y teclado se abren sin acceso de lectura, y los *feature reports*
funcionan igual). Las propiedades PnP son de solo lectura y el autoarranque va en
`HKCU\…\Run`.

**Notificaciones**: en `tauri dev` aparecen con la identidad del ejecutable de
desarrollo. Con el instalador NSIS llevan el nombre de la app, porque el acceso
directo registra el AppUserModelID. El autoarranque activado desde `tauri dev` apunta
al `.exe` de debug: desactívalo antes de borrar `target/`.
