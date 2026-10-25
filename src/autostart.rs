//! Gestión del inicio automático de Nexus Monitor con Windows en el registro de usuario actual (HKCU).

use std::ptr;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const SUBCLAVE_RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const NOMBRE_VALOR: &str = "NexusMonitor";

fn a_wstring(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Configura o elimina la entrada de inicio automático en `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub fn configurar_inicio_automatico(habilitar: bool) -> bool {
    let subclave = a_wstring(SUBCLAVE_RUN);
    let nombre = a_wstring(NOMBRE_VALOR);

    unsafe {
        let mut hkey: HKEY = 0 as _;
        if habilitar {
            let resultado = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subclave.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if resultado != 0 {
                return false;
            }

            if let Ok(exe_path) = std::env::current_exe() {
                let cmd = format!("\"{}\"", exe_path.to_string_lossy());
                let cmd_w = a_wstring(&cmd);
                let tam_bytes = (cmd_w.len() * 2) as u32;

                let res_set = RegSetValueExW(
                    hkey,
                    nombre.as_ptr(),
                    0,
                    REG_SZ,
                    cmd_w.as_ptr() as *const u8,
                    tam_bytes,
                );
                RegCloseKey(hkey);
                res_set == 0
            } else {
                RegCloseKey(hkey);
                false
            }
        } else {
            let resultado = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subclave.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if resultado != 0 {
                return false;
            }

            let _ = RegDeleteValueW(hkey, nombre.as_ptr());
            RegCloseKey(hkey);
            true
        }
    }
}

/// Comprueba si la entrada de inicio automático está actualmente registrada en HKCU.
#[allow(dead_code)]
pub fn esta_registrado_inicio_automatico() -> bool {
    let subclave = a_wstring(SUBCLAVE_RUN);
    let nombre = a_wstring(NOMBRE_VALOR);

    unsafe {
        let mut hkey: HKEY = 0 as _;
        let resultado = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subclave.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if resultado != 0 {
            return false;
        }

        let mut tipo: u32 = 0;
        let mut tam: u32 = 0;
        let res_query = RegQueryValueExW(
            hkey,
            nombre.as_ptr(),
            ptr::null_mut(),
            &mut tipo,
            ptr::null_mut(),
            &mut tam,
        );
        RegCloseKey(hkey);
        res_query == 0
    }
}
