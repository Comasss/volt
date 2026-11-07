//! Detección de Razer Synapse (RazerAppEngine.exe) y aviso cuando se cierra.
//!
//! El aviso no sondea: un hilo queda bloqueado en `WaitForSingleObject` sobre el
//! proceso (0 % de CPU) y, cuando Synapse termina, llama al callback. Así Volt
//! reajusta el auricular al momento en vez de esperar al siguiente sondeo.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const EXE: &str = "RazerAppEngine.exe";
/// Tras cerrarse Synapse, el auricular tarda un poco en volver a su estado propio.
const SETTLE: Duration = Duration::from_secs(3);

static WATCHING: AtomicBool = AtomicBool::new(false);

/// PID del proceso principal de Synapse (Electron lanza varios), si está abierto.
#[cfg(windows)]
pub fn find() -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut engines: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(EXE) {
                    engines.push((e.th32ProcessID, e.th32ParentProcessID));
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    // El principal es el que no cuelga de otro RazerAppEngine.
    engines
        .iter()
        .find(|(_, parent)| !engines.iter().any(|(pid, _)| pid == parent))
        .or(engines.first())
        .map(|(pid, _)| *pid)
}

#[cfg(not(windows))]
pub fn find() -> Option<u32> {
    None
}

pub fn is_running() -> bool {
    find().is_some()
}

/// Llama a `on_exit` cuando termine el proceso `pid`. Un solo vigilante a la vez.
#[cfg(windows)]
pub fn watch_exit(pid: u32, on_exit: impl FnOnce() + Send + 'static) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
    };
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("volt-synapse-watch".into())
        .spawn(move || {
            let waited = unsafe {
                match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                    Ok(h) => {
                        WaitForSingleObject(h, INFINITE);
                        let _ = CloseHandle(h);
                        true
                    }
                    Err(e) => {
                        log::debug!("synapse: no se puede vigilar el PID {pid}: {e}");
                        false
                    }
                }
            };
            if waited {
                log::info!("synapse: cerrado; reajustando auriculares");
                std::thread::sleep(SETTLE);
            }
            WATCHING.store(false, Ordering::SeqCst);
            if waited {
                on_exit();
            }
        });
    if spawned.is_err() {
        WATCHING.store(false, Ordering::SeqCst);
    }
}

#[cfg(not(windows))]
pub fn watch_exit(_pid: u32, _on_exit: impl FnOnce() + Send + 'static) {}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn notifies_when_the_process_exits() {
        // Un proceso cualquiera que vive ~1 s hace de "Synapse".
        let child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 2 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        super::watch_exit(child.id(), move || tx.send(()).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(15)).is_ok());
    }
}
