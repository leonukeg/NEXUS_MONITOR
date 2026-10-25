//! Ventana de configuración visual nativa para Nexus Monitor.
//!
//! Permite personalizar la apariencia y los módulos visibles mediante
//! controles estándar de Windows (checkboxes, botones) sin necesidad
//! de editar manualmente archivos de configuración.

use std::mem;
use std::ptr;

use crate::a_wstring;
use crate::config::{guardar_configuracion, Configuracion};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, BM_GETCHECK, BM_SETCHECK,
    CS_HREDRAW, CS_VREDRAW, IDC_ARROW, MSG, SW_SHOW, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const BST_UNCHECKED: u32 = 0;
const BST_CHECKED: u32 = 1;

static mut VENTANA_CONFIG_HWND: HWND = 0 as _;

// Identificadores de controles UI
const ID_CHK_CPU: usize = 101;
const ID_CHK_RAM: usize = 102;
const ID_CHK_RED: usize = 103;
const ID_CHK_GPU: usize = 104;
const ID_CHK_DISCOS: usize = 105;
const ID_CHK_TEMPS: usize = 106;
const ID_CHK_GRAFICAS: usize = 107;
const ID_CHK_TOP_PROCESOS: usize = 108;
const ID_CHK_AUTOSTART: usize = 109;
const ID_BTN_GUARDAR: usize = 201;
const ID_BTN_CANCELAR: usize = 202;

static mut HWND_CHK_CPU: HWND = 0 as _;
static mut HWND_CHK_RAM: HWND = 0 as _;
static mut HWND_CHK_RED: HWND = 0 as _;
static mut HWND_CHK_GPU: HWND = 0 as _;
static mut HWND_CHK_DISCOS: HWND = 0 as _;
static mut HWND_CHK_TEMPS: HWND = 0 as _;
static mut HWND_CHK_GRAFICAS: HWND = 0 as _;
static mut HWND_CHK_TOP_PROCESOS: HWND = 0 as _;
static mut HWND_CHK_AUTOSTART: HWND = 0 as _;

/// Comprueba si la ventana de configuración está abierta.
pub fn esta_config_abierta() -> bool {
    unsafe { VENTANA_CONFIG_HWND != 0 as _ }
}

/// Abre o enfoca la ventana de configuración visual.
pub fn abrir_ventana_configuracion(config_actual: Configuracion) {
    unsafe {
        if VENTANA_CONFIG_HWND != 0 as _ {
            SetForegroundWindow(VENTANA_CONFIG_HWND);
            return;
        }

        std::thread::spawn(move || {
            iniciar_bucle_configuracion(config_actual);
        });
    }
}

fn iniciar_bucle_configuracion(config: Configuracion) {
    unsafe {
        let nombre_clase = a_wstring("NexusConfigWindowClass");
        let titulo_ventana = a_wstring("Nexus Monitor — Configuración");

    let brush_fondo = CreateSolidBrush(0x00F0F0F0);

    let wnd_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc_config),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0 as _,
        hIcon: 0 as _,
        hCursor: windows_sys::Win32::UI::WindowsAndMessaging::LoadCursorW(0 as _, IDC_ARROW),
        hbrBackground: brush_fondo,
        lpszMenuName: ptr::null(),
        lpszClassName: nombre_clase.as_ptr(),
    };

    RegisterClassW(&wnd_class);

    let hwnd = CreateWindowExW(
        0,
        nombre_clase.as_ptr(),
        titulo_ventana.as_ptr(),
        WS_OVERLAPPEDWINDOW & !windows_sys::Win32::UI::WindowsAndMessaging::WS_THICKFRAME & !windows_sys::Win32::UI::WindowsAndMessaging::WS_MAXIMIZEBOX,
        150,
        150,
        380,
        480,
        0 as _,
        0 as _,
        0 as _,
        ptr::null(),
    );

    if hwnd == 0 as _ {
        return;
    }

    VENTANA_CONFIG_HWND = hwnd;

    // Crear controles nativos:
    let btn_class = a_wstring("BUTTON");
    let static_class = a_wstring("STATIC");

    // Título de la sección
    let txt_sec = a_wstring("Módulos visibles en el panel:");
    CreateWindowExW(
        0,
        static_class.as_ptr(),
        txt_sec.as_ptr(),
        WS_CHILD | WS_VISIBLE,
        24,
        20,
        320,
        20,
        hwnd,
        0 as _,
        0 as _,
        ptr::null(),
    );

    // Helper para crear Checkbox
    let crear_chk = |texto: &str, id: usize, y: i32, activo: bool| -> HWND {
        let wtexto = a_wstring(texto);
        let h = CreateWindowExW(
            0,
            btn_class.as_ptr(),
            wtexto.as_ptr(),
            WS_CHILD | WS_VISIBLE | 0x00000003, // BS_AUTOCHECKBOX = 3
            28,
            y,
            300,
            24,
            hwnd,
            id as _,
            0 as _,
            ptr::null(),
        );
        let check_val = if activo { BST_CHECKED } else { BST_UNCHECKED };
        SendMessageW(h, BM_SETCHECK, check_val as usize, 0);
        h
    };

    HWND_CHK_CPU = crear_chk("Mostrar CPU y Frecuencia", ID_CHK_CPU, 46, config.modulos.mostrar_cpu);
    HWND_CHK_RAM = crear_chk("Mostrar Memoria RAM", ID_CHK_RAM, 74, config.modulos.mostrar_ram);
    HWND_CHK_RED = crear_chk("Mostrar Red, Tráfico y VPN", ID_CHK_RED, 102, config.modulos.mostrar_red);
    HWND_CHK_GPU = crear_chk("Mostrar GPU y Memoria VRAM", ID_CHK_GPU, 130, config.modulos.mostrar_gpu);
    HWND_CHK_DISCOS = crear_chk("Mostrar Almacenamiento y Discos", ID_CHK_DISCOS, 158, config.modulos.mostrar_discos);
    HWND_CHK_TEMPS = crear_chk("Mostrar Sensores Térmicos", ID_CHK_TEMPS, 186, config.modulos.mostrar_temperaturas);
    HWND_CHK_GRAFICAS = crear_chk("Mostrar Gráficas Históricas (Sparklines)", ID_CHK_GRAFICAS, 214, config.modulos.mostrar_graficas);
    HWND_CHK_TOP_PROCESOS = crear_chk("Mostrar Top 3 Procesos (RAM/CPU)", ID_CHK_TOP_PROCESOS, 242, config.modulos.mostrar_top_procesos);

    // Separador y opción de inicio automático
    HWND_CHK_AUTOSTART = crear_chk("Iniciar Nexus con Windows (Inicio automático)", ID_CHK_AUTOSTART, 280, config.comportamiento.iniciar_con_windows);

    // Botón Guardar
    let txt_guardar = a_wstring("Guardar y Aplicar");
    CreateWindowExW(
        0,
        btn_class.as_ptr(),
        txt_guardar.as_ptr(),
        WS_CHILD | WS_VISIBLE | 0x00000001, // BS_DEFPUSHBUTTON
        38,
        330,
        140,
        34,
        hwnd,
        ID_BTN_GUARDAR as _,
        0 as _,
        ptr::null(),
    );

    // Botón Cancelar
    let txt_cancelar = a_wstring("Cerrar");
    CreateWindowExW(
        0,
        btn_class.as_ptr(),
        txt_cancelar.as_ptr(),
        WS_CHILD | WS_VISIBLE,
        196,
        330,
        120,
        34,
        hwnd,
        ID_BTN_CANCELAR as _,
        0 as _,
        ptr::null(),
    );

    ShowWindow(hwnd, SW_SHOW);

    let mut msg: MSG = mem::zeroed();
    while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

        VENTANA_CONFIG_HWND = 0 as _;
    }
}

unsafe extern "system" fn wnd_proc_config(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam & 0xFFFF) as usize;
                if id == ID_BTN_GUARDAR {
                    // Leer estados de checkboxes
                    let mut config = crate::config::cargar_o_crear_por_defecto();

                    config.modulos.mostrar_cpu = SendMessageW(HWND_CHK_CPU, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_ram = SendMessageW(HWND_CHK_RAM, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_red = SendMessageW(HWND_CHK_RED, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_gpu = SendMessageW(HWND_CHK_GPU, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_discos = SendMessageW(HWND_CHK_DISCOS, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_temperaturas = SendMessageW(HWND_CHK_TEMPS, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_graficas = SendMessageW(HWND_CHK_GRAFICAS, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.modulos.mostrar_top_procesos = SendMessageW(HWND_CHK_TOP_PROCESOS, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;

                    let autostart = SendMessageW(HWND_CHK_AUTOSTART, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED;
                    config.comportamiento.iniciar_con_windows = autostart;
                    crate::autostart::configurar_inicio_automatico(autostart);

                    guardar_configuracion(&config);
                    crate::notificar_actualizacion_configuracion(config);

                    println!("[Nexus] Configuración guardada y aplicada con éxito desde la interfaz.");
                    DestroyWindow(hwnd);
                } else if id == ID_BTN_CANCELAR {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
