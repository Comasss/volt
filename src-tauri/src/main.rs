// En release no queremos consola detrás de la app de bandeja.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    volt_lib::run()
}
