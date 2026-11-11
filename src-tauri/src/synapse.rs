//! Detección de Razer Synapse (RazerAppEngine.exe) y aviso cuando se cierra.
//!
//! El aviso no sondea: un hilo queda bloqueado en `WaitForSingleObject` sobre el
//! proceso (0 % de CPU) y, cuando Synapse termina, llama al callback. Así Volt
//! reajusta el auricular al momento en vez de esperar al siguiente sondeo.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::Duration;

const EXE: &str = "RazerAppEngine.exe";
/// Tras cerrarse Synapse, el auricular tarda un poco en volver a su estado propio.
#[cfg(windows)]
const SETTLE: Duration = Duration::from_secs(3);

#[cfg(windows)]
static WATCHING: AtomicBool = AtomicBool::new(false);

/// (PID, PID del padre) de todos los procesos con ese nombre de ejecutable.
#[cfg(windows)]
fn processes(exe: &str) -> Vec<(u32, u32)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut found: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                    found.push((e.th32ProcessID, e.th32ParentProcessID));
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    found
}

#[cfg(not(windows))]
fn processes(_exe: &str) -> Vec<(u32, u32)> {
    Vec::new()
}

/// ¿Hay algún proceso con ese nombre de ejecutable? (p. ej. `lghub_agent.exe`).
pub fn process_running(exe: &str) -> bool {
    !processes(exe).is_empty()
}

/// PID del proceso principal de Synapse (Electron lanza varios), si está abierto.
pub fn find() -> Option<u32> {
    let engines = processes(EXE);
    // El principal es el que no cuelga de otro RazerAppEngine.
    engines
        .iter()
        .find(|(_, parent)| !engines.iter().any(|(pid, _)| pid == parent))
        .or(engines.first())
        .map(|(pid, _)| *pid)
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
