#![windows_subsystem = "windows"]

//! Nexus Monitor v0.4
//!
//! Monitor de escritorio nativo para Windows:
//! - CPU (Uso %, frecuencia en GHz, gráfica Sparkline).
//! - Memoria RAM (Usada, Total, %, barra de progreso).
//! - Red y VPN (Adaptador activo, IPv4, velocidades ↓ / ↑, estado VPN).
//! - GPU y VRAM (Adaptador, uso %, memoria dedicada/compartida, temperatura).
//! - Almacenamiento / Discos (Unidad C:, espacio usado/total, velocidades L/E).
//! - Sensores térmicos.
//! - Ventana nativa de configuración visual accesible con el botón [⚙].

mod autostart;
mod config;
mod disks;
mod gpu;
mod metrics;
mod network;
mod processes;
mod temperature;
mod ui_config;

use std::mem;
use std::ptr;
use std::sync::Mutex;

use config::{Configuracion, cargar_o_crear_por_defecto, guardar_posicion_en_disco, hex_a_bgr};
use disks::{EstadoDiscos, RecolectorDiscos};
use gpu::{InfoGPU, RecolectorGPU};
use metrics::{MemoriaInfo, RecolectorMetricas};
use network::{EstadoVPN, InfoRed, RecolectorRed, TipoConexion, formatear_velocidad};
use processes::{InfoProceso, ModoTopProcesos, RecolectorProcesos};
use temperature::{InfoTemperaturas, RecolectorTemperaturas};
use ui_config::abrir_ventana_configuracion;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateRoundRectRgn, CreateSolidBrush, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject, HDC,
    InvalidateRect, NULL_PEN, PAINTSTRUCT, PS_SOLID, Polygon, Polyline, SRCCOPY, ScreenToClient,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics, HTCAPTION, HTCLIENT, IDC_ARROW,
    KillTimer, LWA_ALPHA, LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SW_SHOW,
    SWP_NOMOVE, SWP_NOZORDER, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
    SetWindowPos, ShowWindow, TranslateMessage, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_DEVICECHANGE,
    WM_LBUTTONUP, WM_MOVE, WM_NCHITTEST, WM_NCMOUSELEAVE, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_POPUP, WS_SYSMENU, WS_VISIBLE,
};

const WM_MOUSELEAVE: u32 = 0x02A3;
const TME_LEAVE: u32 = 0x00000002;
const TME_NONCLIENT: u32 = 0x00000010;

#[repr(C)]
#[allow(non_snake_case)]
struct TRACKMOUSEEVENT {
    cbSize: u32,
    dwFlags: u32,
    hwndTrack: HWND,
    dwHoverTime: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn TrackMouseEvent(lpEventTrack: *mut TRACKMOUSEEVENT) -> windows_sys::Win32::Foundation::BOOL;
}

/// Capacidad del historial para las gráficas Sparkline.
const MUESTRAS_HISTORIAL: usize = 28;

/// Estados de la barra superior inteligente oculta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EstadoBarraSuperior {
    Oculta,
    EsperandoAparicion,
    Visible,
    EsperandoOcultacion,
}

/// Estado global de la aplicación protegido para acceso concurrente seguro.
struct EstadoApp {
    recolector_metricas: RecolectorMetricas,
    recolector_red: RecolectorRed,
    recolector_gpu: RecolectorGPU,
    recolector_discos: RecolectorDiscos,
    recolector_temperaturas: RecolectorTemperaturas,
    recolector_procesos: RecolectorProcesos,
    config: Configuracion,
    uso_cpu: f64,
    frecuencia_cpu_ghz: Option<f64>,
    historial_cpu: Vec<f64>,
    memoria: Option<MemoriaInfo>,
    red: InfoRed,
    gpu: InfoGPU,
    historial_gpu: Vec<f64>,
    discos: EstadoDiscos,
    top_procesos: Vec<InfoProceso>,
    modo_procesos: ModoTopProcesos,
    tick_procesos: u32,
    y_selector_procesos: i32,
    temperaturas: InfoTemperaturas,
    estado_barra: EstadoBarraSuperior,
    barra_visible: bool,
    hover_cerrar: bool,
    hover_config: bool,
    posicion_actual: (i32, i32),
    ancho_panel: i32,
    alto_panel: i32,
    hwnd_principal: isize,
}

static ESTADO: Mutex<Option<EstadoApp>> = Mutex::new(None);

/// Trunca un texto si excede `max_caracteres`, agregando ".." al final sin desbordar.
fn truncar_texto(texto: &str, max_caracteres: usize) -> String {
    if texto.chars().count() > max_caracteres {
        let trunc: String = texto
            .chars()
            .take(max_caracteres.saturating_sub(2))
            .collect();
        format!("{}..", trunc)
    } else {
        texto.to_string()
    }
}

/// Identificador numérico del temporizador de actualización periódica.
const ID_TEMPORIZADOR: usize = 1;
/// Identificador del temporizador de 1 segundo para mostrar la barra tras hover en la zona superior.
const ID_TIMER_MOSTRAR_BARRA: usize = 2;
/// Identificador del temporizador con retardo de 600 ms para ocultar la barra tras salida.
const ID_TIMER_OCULTAR_BARRA: usize = 3;

/// Convierte una cadena de texto en un array UTF-16 terminado en cero para la API de Windows.
pub fn a_wstring(texto: &str) -> Vec<u16> {
    texto.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Dibuja un rectángulo con un color sólido en el contexto de dispositivo (HDC).
fn rellenar_rect(hdc: HDC, left: i32, top: i32, right: i32, bottom: i32, color_bgr: u32) {
    unsafe {
        let rect = RECT {
            left,
            top,
            right,
            bottom,
        };
        let brush = CreateSolidBrush(color_bgr);
        FillRect(hdc, &rect, brush);
        DeleteObject(brush);
    }
}

/// Dibuja una línea de texto con alineación y color específicos.
fn dibujar_texto(
    hdc: HDC,
    texto: &str,
    rect: &mut RECT,
    color_bgr: u32,
    alineacion_derecha: bool,
    centrado: bool,
) {
    unsafe {
        let wtexto = a_wstring(texto);
        SetTextColor(hdc, color_bgr);

        let flag_alineacion = if centrado {
            DT_CENTER
        } else if alineacion_derecha {
            DT_RIGHT
        } else {
            DT_LEFT
        };

        DrawTextW(
            hdc,
            wtexto.as_ptr(),
            (wtexto.len() - 1) as i32,
            rect,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | flag_alineacion,
        );
    }
}

/// Mezcla dos colores BGR según un factor de interpolación [0.0, 1.0].
fn mezclar_color_bgr(color_base: u32, color_tinte: u32, factor: f64) -> u32 {
    let b_base = (color_base & 0xFF) as f64;
    let g_base = ((color_base >> 8) & 0xFF) as f64;
    let r_base = ((color_base >> 16) & 0xFF) as f64;

    let b_tinte = (color_tinte & 0xFF) as f64;
    let g_tinte = ((color_tinte >> 8) & 0xFF) as f64;
    let r_tinte = ((color_tinte >> 16) & 0xFF) as f64;

    let b = (b_base + (b_tinte - b_base) * factor)
        .round()
        .clamp(0.0, 255.0) as u32;
    let g = (g_base + (g_tinte - g_base) * factor)
        .round()
        .clamp(0.0, 255.0) as u32;
    let r = (r_base + (r_tinte - r_base) * factor)
        .round()
        .clamp(0.0, 255.0) as u32;

    (r << 16) | (g << 8) | b
}

/// Dibuja una gráfica de líneas continua (Sparkline) con trazo fino de 1px y relleno suave bajo la curva.
fn dibujar_grafica_linea(
    hdc: HDC,
    historial: &[f64],
    left: i32,
    top: i32,
    ancho: i32,
    alto: i32,
    color_linea_bgr: u32,
    color_fondo_tarjeta_bgr: u32,
) {
    if historial.len() < 2 || ancho <= 0 || alto <= 0 {
        return;
    }

    unsafe {
        let total = historial.len();
        let paso_x = (ancho as f64) / ((total - 1) as f64);

        let mut puntos: Vec<POINT> = Vec::with_capacity(total);

        for (i, &val) in historial.iter().enumerate() {
            let px = left + (i as f64 * paso_x).round() as i32;
            let fraccion = (val.clamp(0.0, 100.0) / 100.0) as f64;
            let py = (top + alto) - (fraccion * (alto as f64 - 4.0)).round() as i32 - 2;
            puntos.push(POINT { x: px, y: py });
        }

        // Relleno suave bajo la curva mediante polígono cerrado
        let mut poly_puntos: Vec<POINT> = Vec::with_capacity(total + 2);
        poly_puntos.extend_from_slice(&puntos);
        poly_puntos.push(POINT {
            x: puntos.last().unwrap().x,
            y: top + alto,
        });
        poly_puntos.push(POINT {
            x: puntos.first().unwrap().x,
            y: top + alto,
        });

        let color_relleno = mezclar_color_bgr(color_fondo_tarjeta_bgr, color_linea_bgr, 0.22);
        let brush_relleno = CreateSolidBrush(color_relleno);
        let null_pen = GetStockObject(NULL_PEN);

        let brush_ant = SelectObject(hdc, brush_relleno);
        let pen_ant = SelectObject(hdc, null_pen);

        Polygon(hdc, poly_puntos.as_ptr(), poly_puntos.len() as i32);

        // Trazo fino y minimalista de 1px para la curva
        let pen_linea = CreatePen(PS_SOLID, 1, color_linea_bgr);
        SelectObject(hdc, pen_linea);

        Polyline(hdc, puntos.as_ptr(), puntos.len() as i32);

        // Limpieza y restauración de objetos GDI
        SelectObject(hdc, pen_ant);
        SelectObject(hdc, brush_ant);
        DeleteObject(pen_linea);
        DeleteObject(brush_relleno);
    }
}

/// Dibuja el panel completo con todos los módulos activos.
fn pintar_panel(
    hdc: HDC,
    ancho: i32,
    alto: i32,
    uso_cpu: f64,
    frecuencia_cpu_ghz: Option<f64>,
    historial_cpu: &[f64],
    memoria: Option<MemoriaInfo>,
    red: &InfoRed,
    gpu: &InfoGPU,
    _historial_gpu: &[f64],
    discos: &EstadoDiscos,
    top_procesos: &[InfoProceso],
    modo_procesos: ModoTopProcesos,
    temperaturas: &InfoTemperaturas,
    hover_cerrar: bool,
    hover_config: bool,
    barra_visible: bool,
    config: &Configuracion,
) -> i32 {
    let mut y_sel_top_proc = 0;

    unsafe {
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, ancho, alto);
        let bmp_anterior = SelectObject(mem_dc, mem_bmp);

        SetBkMode(mem_dc, TRANSPARENT as i32);

        // Colores
        let color_fondo = hex_a_bgr(&config.colores.fondo);
        let color_tarjeta = hex_a_bgr(&config.colores.fondo_tarjeta);
        let color_borde = hex_a_bgr(&config.colores.borde);
        let color_texto_principal = hex_a_bgr(&config.colores.texto_principal);
        let color_texto_secundario = hex_a_bgr(&config.colores.texto_secundario);

        let color_acento_cpu = if uso_cpu >= 75.0 {
            hex_a_bgr(&config.colores.acento_cpu_alto)
        } else {
            hex_a_bgr(&config.colores.acento_cpu)
        };

        let color_acento_ram = hex_a_bgr(&config.colores.acento_ram);
        let color_acento_descarga = hex_a_bgr(&config.colores.acento_descarga);
        let color_acento_subida = hex_a_bgr(&config.colores.acento_subida);
        let color_acento_gpu = hex_a_bgr(&config.colores.acento_gpu);
        let color_acento_disco = hex_a_bgr(&config.colores.acento_disco);

        // Fondo y bordes
        rellenar_rect(mem_dc, 0, 0, ancho, alto, color_fondo);
        rellenar_rect(mem_dc, 0, 0, ancho, 1, color_borde);
        rellenar_rect(mem_dc, 0, alto - 1, ancho, alto, color_borde);
        rellenar_rect(mem_dc, 0, 0, 1, alto, color_borde);
        rellenar_rect(mem_dc, ancho - 1, 0, ancho, alto, color_borde);

        // Fuentes unificadas, limpias y finas (peso normal 400 en todas, sin negritas)
        let font_nombre = a_wstring(&config.tipografia.fuente);
        let tam_base = config.tipografia.tamano_base;

        let font_encabezado = CreateFontW(
            tam_base - 1,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            2,
            font_nombre.as_ptr(),
        );
        let font_etiquetas = CreateFontW(
            tam_base,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            2,
            font_nombre.as_ptr(),
        );
        let font_valores = CreateFontW(
            tam_base,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            2,
            font_nombre.as_ptr(),
        );
        let font_detalle = CreateFontW(
            tam_base - 1,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            2,
            font_nombre.as_ptr(),
        );
        let font_flechas = CreateFontW(
            tam_base,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            2,
            font_nombre.as_ptr(),
        );

        let pad_x = 16;
        let ancho_util = ancho - (pad_x * 2);

        // Inicio directo en y=10 para máxima compacidad y posición estable de métricas
        let mut y_cursor = 10;

        // --- BLOQUE CPU ---
        if config.modulos.mostrar_cpu {
            let alto_card_cpu = if config.modulos.mostrar_graficas {
                76
            } else {
                38
            };
            rellenar_rect(
                mem_dc,
                pad_x,
                y_cursor,
                pad_x + ancho_util,
                y_cursor + alto_card_cpu,
                color_tarjeta,
            );
            rellenar_rect(
                mem_dc,
                pad_x,
                y_cursor,
                pad_x + ancho_util,
                y_cursor + 1,
                color_borde,
            );

            SelectObject(mem_dc, font_etiquetas);
            let mut rect_cpu_label = RECT {
                left: pad_x + 10,
                top: y_cursor + 8,
                right: pad_x + 150,
                bottom: y_cursor + 24,
            };

            let etiqueta_cpu = match frecuencia_cpu_ghz {
                Some(ghz) => format!("⚡ CPU  {:.2} GHz", ghz),
                None => "⚡ CPU".to_string(),
            };
            dibujar_texto(
                mem_dc,
                &etiqueta_cpu,
                &mut rect_cpu_label,
                color_texto_principal,
                false,
                false,
            );

            SelectObject(mem_dc, font_valores);
            let mut rect_cpu_val = RECT {
                left: pad_x + ancho_util - 80,
                top: y_cursor + 8,
                right: pad_x + ancho_util - 10,
                bottom: y_cursor + 24,
            };
            let texto_cpu = format!("{:>4.1}%", uso_cpu);
            dibujar_texto(
                mem_dc,
                &texto_cpu,
                &mut rect_cpu_val,
                color_acento_cpu,
                true,
                false,
            );

            if config.modulos.mostrar_graficas {
                let gx = pad_x + 8;
                let gy = y_cursor + 28;
                let gw = ancho_util - 16;
                let gh = 40;
                rellenar_rect(mem_dc, gx, gy + gh - 1, gx + gw, gy + gh, 0x002B2420);
                dibujar_grafica_linea(
                    mem_dc,
                    historial_cpu,
                    gx,
                    gy,
                    gw,
                    gh,
                    color_acento_cpu,
                    color_tarjeta,
                );
            }

            y_cursor += alto_card_cpu + 10;
        }

        // --- BLOQUE RAM ---
        if config.modulos.mostrar_ram {
            SelectObject(mem_dc, font_etiquetas);
            let mut rect_ram_label = RECT {
                left: pad_x,
                top: y_cursor,
                right: pad_x + 130,
                bottom: y_cursor + 18,
            };
            let (ram_pct, texto_ram) = match memoria {
                Some(m) => (
                    m.porcentaje_uso,
                    format!("{:.1} / {:.1} GB", m.usada_gb, m.total_gb),
                ),
                None => (0.0, "Consultando...".to_string()),
            };
            let etiqueta_ram = format!("▦ RAM: {:.0}%", ram_pct);
            dibujar_texto(
                mem_dc,
                &etiqueta_ram,
                &mut rect_ram_label,
                color_texto_principal,
                false,
                false,
            );

            SelectObject(mem_dc, font_detalle);
            let mut rect_ram_val = RECT {
                left: pad_x + ancho_util - 120,
                top: y_cursor + 2,
                right: pad_x + ancho_util,
                bottom: y_cursor + 18,
            };
            dibujar_texto(
                mem_dc,
                &texto_ram,
                &mut rect_ram_val,
                color_texto_secundario,
                true,
                false,
            );

            let y_barra_ram = y_cursor + 20;
            let alto_barra = 4;
            let fondo_barra = 0x0026221E;
            rellenar_rect(
                mem_dc,
                pad_x,
                y_barra_ram,
                pad_x + ancho_util,
                y_barra_ram + alto_barra,
                fondo_barra,
            );

            let relleno_ram_w =
                ((ancho_util as f64) * (ram_pct.clamp(0.0, 100.0) / 100.0)).round() as i32;
            if relleno_ram_w > 0 {
                rellenar_rect(
                    mem_dc,
                    pad_x,
                    y_barra_ram,
                    pad_x + relleno_ram_w,
                    y_barra_ram + alto_barra,
                    color_acento_ram,
                );
            }

            y_cursor += 30;
        }

        // --- BLOQUE GPU ---
        if config.modulos.mostrar_gpu {
            SelectObject(mem_dc, font_etiquetas);
            let mut rect_gpu_label = RECT {
                left: pad_x,
                top: y_cursor,
                right: pad_x + 130,
                bottom: y_cursor + 18,
            };
            let etiqueta_gpu = format!("🎮 GPU: {:.0}%", gpu.uso_porcentaje);
            dibujar_texto(
                mem_dc,
                &etiqueta_gpu,
                &mut rect_gpu_label,
                color_texto_principal,
                false,
                false,
            );

            SelectObject(mem_dc, font_detalle);
            let mut rect_gpu_vram = RECT {
                left: pad_x + ancho_util - 130,
                top: y_cursor + 2,
                right: pad_x + ancho_util,
                bottom: y_cursor + 18,
            };
            let texto_vram = format!(
                "{:.0} / {:.0} MB",
                gpu.vram_dedicada_usada_mb, gpu.vram_dedicada_total_mb
            );
            dibujar_texto(
                mem_dc,
                &texto_vram,
                &mut rect_gpu_vram,
                color_texto_secundario,
                true,
                false,
            );

            let y_barra_gpu = y_cursor + 20;
            let alto_barra = 4;
            let fondo_barra = 0x0026221E;
            rellenar_rect(
                mem_dc,
                pad_x,
                y_barra_gpu,
                pad_x + ancho_util,
                y_barra_gpu + alto_barra,
                fondo_barra,
            );

            let relleno_gpu_w = ((ancho_util as f64)
                * (gpu.uso_porcentaje.clamp(0.0, 100.0) / 100.0))
                .round() as i32;
            if relleno_gpu_w > 0 {
                rellenar_rect(
                    mem_dc,
                    pad_x,
                    y_barra_gpu,
                    pad_x + relleno_gpu_w,
                    y_barra_gpu + alto_barra,
                    color_acento_gpu,
                );
            }

            y_cursor += 30;
        }

        // --- BLOQUE RED ---
        if config.modulos.mostrar_red {
            SelectObject(mem_dc, font_detalle);
            let icono_tipo = match red.tipo {
                TipoConexion::WiFi => "📶 Wi-Fi",
                TipoConexion::Ethernet => "🔌 Ethernet",
                TipoConexion::VPN => "🛡 VPN",
                _ => "🌐 Red",
            };

            let vpn_badge = match red.estado_vpn {
                EstadoVPN::ConectadaEncaminada => " • VPN Activa",
                EstadoVPN::DetectadaDirecta => " • VPN Directa",
                EstadoVPN::NoDetectada => "",
            };

            let texto_red = if red.conectado {
                format!("{} • {}{}", icono_tipo, red.ipv4, vpn_badge)
            } else {
                "Sin conexión".to_string()
            };

            let mut rect_sub_red = RECT {
                left: pad_x,
                top: y_cursor,
                right: pad_x + ancho_util,
                bottom: y_cursor + 16,
            };
            dibujar_texto(
                mem_dc,
                &texto_red,
                &mut rect_sub_red,
                color_texto_secundario,
                false,
                false,
            );
            y_cursor += 18;

            // Descarga
            SelectObject(mem_dc, font_flechas);
            let mut rect_f_down = RECT {
                left: pad_x,
                top: y_cursor + 1,
                right: pad_x + 16,
                bottom: y_cursor + 19,
            };
            dibujar_texto(
                mem_dc,
                "↓",
                &mut rect_f_down,
                color_acento_descarga,
                false,
                false,
            );

            SelectObject(mem_dc, font_etiquetas);
            let mut rect_lbl_down = RECT {
                left: pad_x + 18,
                top: y_cursor + 1,
                right: pad_x + 100,
                bottom: y_cursor + 19,
            };
            dibujar_texto(
                mem_dc,
                "Descarga",
                &mut rect_lbl_down,
                color_texto_secundario,
                false,
                false,
            );

            SelectObject(mem_dc, font_valores);
            let mut rect_v_down = RECT {
                left: pad_x + ancho_util - 120,
                top: y_cursor + 1,
                right: pad_x + ancho_util,
                bottom: y_cursor + 19,
            };
            let txt_vel_down = formatear_velocidad(red.velocidad_descarga_bps);
            dibujar_texto(
                mem_dc,
                &txt_vel_down,
                &mut rect_v_down,
                color_texto_principal,
                true,
                false,
            );
            y_cursor += 22;

            // Subida
            SelectObject(mem_dc, font_flechas);
            let mut rect_f_up = RECT {
                left: pad_x,
                top: y_cursor + 1,
                right: pad_x + 16,
                bottom: y_cursor + 19,
            };
            dibujar_texto(
                mem_dc,
                "↑",
                &mut rect_f_up,
                color_acento_subida,
                false,
                false,
            );

            SelectObject(mem_dc, font_etiquetas);
            let mut rect_lbl_up = RECT {
                left: pad_x + 18,
                top: y_cursor + 1,
                right: pad_x + 100,
                bottom: y_cursor + 19,
            };
            dibujar_texto(
                mem_dc,
                "Subida",
                &mut rect_lbl_up,
                color_texto_secundario,
                false,
                false,
            );

            SelectObject(mem_dc, font_valores);
            let mut rect_v_up = RECT {
                left: pad_x + ancho_util - 120,
                top: y_cursor + 1,
                right: pad_x + ancho_util,
                bottom: y_cursor + 19,
            };
            let txt_vel_up = formatear_velocidad(red.velocidad_subida_bps);
            dibujar_texto(
                mem_dc,
                &txt_vel_up,
                &mut rect_v_up,
                color_texto_principal,
                true,
                false,
            );
            y_cursor += 28;
        }

        // --- BLOQUE DISCOS (Distribución estable en 2 líneas sin solapamientos) ---
        if config.modulos.mostrar_discos {
            if discos.unidades.is_empty() {
                SelectObject(mem_dc, font_etiquetas);
                let mut rect_d_label = RECT {
                    left: pad_x,
                    top: y_cursor,
                    right: pad_x + ancho_util,
                    bottom: y_cursor + 18,
                };
                dibujar_texto(
                    mem_dc,
                    "💾 Discos: sin unidades detectadas",
                    &mut rect_d_label,
                    color_texto_secundario,
                    false,
                    false,
                );
                y_cursor += 30;
            } else {
                let max_mostrar = discos.unidades.len().min(4);
                for disco in discos.unidades.iter().take(max_mostrar) {
                    // Línea 1: Etiqueta/Tipo a la izquierda, Porcentaje a la derecha
                    let etiqueta_disco = if disco.etiqueta.is_empty() {
                        format!("💾 Disco {} — {}", disco.letra, disco.tipo_medio)
                    } else {
                        let etiqueta_corta = truncar_texto(&disco.etiqueta, 13);
                        format!(
                            "💾 Disco {} ({}) — {}",
                            disco.letra, etiqueta_corta, disco.tipo_medio
                        )
                    };

                    SelectObject(mem_dc, font_etiquetas);
                    let mut rect_d_label = RECT {
                        left: pad_x,
                        top: y_cursor,
                        right: pad_x + ancho_util - 48,
                        bottom: y_cursor + 17,
                    };
                    dibujar_texto(
                        mem_dc,
                        &etiqueta_disco,
                        &mut rect_d_label,
                        color_texto_principal,
                        false,
                        false,
                    );

                    SelectObject(mem_dc, font_valores);
                    let mut rect_d_pct = RECT {
                        left: pad_x + ancho_util - 46,
                        top: y_cursor,
                        right: pad_x + ancho_util,
                        bottom: y_cursor + 17,
                    };
                    let txt_pct = format!("{:.0}%", disco.porcentaje_uso);
                    dibujar_texto(
                        mem_dc,
                        &txt_pct,
                        &mut rect_d_pct,
                        color_acento_disco,
                        true,
                        false,
                    );

                    // Línea 2: Barra de progreso a todo el ancho útil
                    let y_barra_d = y_cursor + 19;
                    let alto_barra = 4;
                    let fondo_barra = 0x0026221E;
                    rellenar_rect(
                        mem_dc,
                        pad_x,
                        y_barra_d,
                        pad_x + ancho_util,
                        y_barra_d + alto_barra,
                        fondo_barra,
                    );

                    let relleno_d_w = ((ancho_util as f64)
                        * (disco.porcentaje_uso.clamp(0.0, 100.0) / 100.0))
                        .round() as i32;
                    if relleno_d_w > 0 {
                        rellenar_rect(
                            mem_dc,
                            pad_x,
                            y_barra_d,
                            pad_x + relleno_d_w,
                            y_barra_d + alto_barra,
                            color_acento_disco,
                        );
                    }

                    // Línea 3: Desglose de capacidad (usado a la izquierda, libre a la derecha)
                    SelectObject(mem_dc, font_detalle);
                    let mut rect_d_usado = RECT {
                        left: pad_x,
                        top: y_cursor + 25,
                        right: pad_x + ancho_util - 90,
                        bottom: y_cursor + 39,
                    };
                    let txt_usado = format!(
                        "{:.0} GB usados de {:.0} GB",
                        disco.usado_gb, disco.total_gb
                    );
                    dibujar_texto(
                        mem_dc,
                        &txt_usado,
                        &mut rect_d_usado,
                        color_texto_secundario,
                        false,
                        false,
                    );

                    let mut rect_d_libre = RECT {
                        left: pad_x + ancho_util - 90,
                        top: y_cursor + 25,
                        right: pad_x + ancho_util,
                        bottom: y_cursor + 39,
                    };
                    let txt_libre = format!("{:.0} GB libres", disco.libre_gb);
                    dibujar_texto(
                        mem_dc,
                        &txt_libre,
                        &mut rect_d_libre,
                        color_texto_secundario,
                        true,
                        false,
                    );

                    y_cursor += 44;
                }
            }
            y_cursor += 2;
        }

        // --- BLOQUE TOP PROCESOS (Módulo opcional con selector RAM / CPU) ---
        if config.modulos.mostrar_top_procesos {
            y_sel_top_proc = y_cursor;

            // Cabecera: Título a la izquierda y selector RAM/CPU a la derecha
            SelectObject(mem_dc, font_etiquetas);
            let mut rect_tp_label = RECT {
                left: pad_x,
                top: y_cursor,
                right: pad_x + 130,
                bottom: y_cursor + 18,
            };
            dibujar_texto(
                mem_dc,
                "⚡ Top Procesos",
                &mut rect_tp_label,
                color_texto_principal,
                false,
                false,
            );

            let txt_modo = match modo_procesos {
                ModoTopProcesos::Ram => "[RAM]  CPU",
                ModoTopProcesos::Cpu => " RAM  [CPU]",
            };
            SelectObject(mem_dc, font_detalle);
            let mut rect_tp_sel = RECT {
                left: pad_x + ancho_util - 85,
                top: y_cursor,
                right: pad_x + ancho_util,
                bottom: y_cursor + 18,
            };
            let color_sel = match modo_procesos {
                ModoTopProcesos::Ram => color_acento_ram,
                ModoTopProcesos::Cpu => color_acento_cpu,
            };
            dibujar_texto(mem_dc, txt_modo, &mut rect_tp_sel, color_sel, true, false);

            y_cursor += 20;

            if top_procesos.is_empty() {
                SelectObject(mem_dc, font_detalle);
                let mut rect_tp_vacio = RECT {
                    left: pad_x,
                    top: y_cursor,
                    right: pad_x + ancho_util,
                    bottom: y_cursor + 16,
                };
                dibujar_texto(
                    mem_dc,
                    "Analizando procesos...",
                    &mut rect_tp_vacio,
                    color_texto_secundario,
                    false,
                    false,
                );
                y_cursor += 18;
            } else {
                for (idx, proc) in top_procesos.iter().enumerate() {
                    let nombre_proc = truncar_texto(&proc.nombre, 16);
                    let texto_proc = format!("{}. {}", idx + 1, nombre_proc);

                    SelectObject(mem_dc, font_detalle);
                    let mut rect_p_nom = RECT {
                        left: pad_x,
                        top: y_cursor,
                        right: pad_x + ancho_util - 75,
                        bottom: y_cursor + 16,
                    };
                    dibujar_texto(
                        mem_dc,
                        &texto_proc,
                        &mut rect_p_nom,
                        color_texto_principal,
                        false,
                        false,
                    );

                    let valor_proc = match modo_procesos {
                        ModoTopProcesos::Ram => proc.memoria_formateada(),
                        ModoTopProcesos::Cpu => proc.cpu_formateada(),
                    };

                    let color_val = match modo_procesos {
                        ModoTopProcesos::Ram => color_acento_ram,
                        ModoTopProcesos::Cpu => color_acento_cpu,
                    };

                    let mut rect_p_val = RECT {
                        left: pad_x + ancho_util - 75,
                        top: y_cursor,
                        right: pad_x + ancho_util,
                        bottom: y_cursor + 16,
                    };
                    dibujar_texto(mem_dc, &valor_proc, &mut rect_p_val, color_val, true, false);

                    y_cursor += 17;
                }
            }
            y_cursor += 8;
        }

        // --- BLOQUE TEMPERATURAS ---
        if config.modulos.mostrar_temperaturas {
            SelectObject(mem_dc, font_detalle);
            let txt_cpu_temp = match temperaturas.cpu_celsius {
                Some(t) => format!("{:.0} °C", t),
                None => "No disp.".to_string(),
            };
            let txt_gpu_temp = match temperaturas.gpu_celsius {
                Some(t) => format!("{:.0} °C", t),
                None => "No disp.".to_string(),
            };

            let texto_termico = format!("🌡 CPU: {}   |   GPU: {}", txt_cpu_temp, txt_gpu_temp);

            let mut rect_temp = RECT {
                left: pad_x,
                top: y_cursor,
                right: pad_x + ancho_util,
                bottom: y_cursor + 18,
            };
            dibujar_texto(
                mem_dc,
                &texto_termico,
                &mut rect_temp,
                color_texto_secundario,
                false,
                false,
            );
        }

        // --- BARRA SUPERIOR FLOTANTE INTELIGENTE (OCULTA POR DEFECTO) ---
        if barra_visible {
            // Fondo flotante de la barra sobre el panel superior
            rellenar_rect(mem_dc, 0, 0, ancho, 28, color_fondo);
            rellenar_rect(mem_dc, 0, 27, ancho, 28, color_borde);

            // Título
            SelectObject(mem_dc, font_encabezado);
            let mut rect_encabezado = RECT {
                left: pad_x,
                top: 7,
                right: ancho - 56,
                bottom: 23,
            };
            dibujar_texto(
                mem_dc,
                "NEXUS MONITOR",
                &mut rect_encabezado,
                color_texto_secundario,
                false,
                false,
            );

            // Botón [⚙] Configuración
            let x_cfg_l = ancho - 48;
            let y_cfg_t = 6;
            let x_cfg_r = ancho - 30;
            let y_cfg_b = 24;

            let color_cfg = if hover_config {
                color_texto_principal
            } else {
                color_texto_secundario
            };
            SelectObject(mem_dc, font_etiquetas);
            let mut rect_btn_cfg = RECT {
                left: x_cfg_l,
                top: y_cfg_t,
                right: x_cfg_r,
                bottom: y_cfg_b,
            };
            dibujar_texto(mem_dc, "⚙", &mut rect_btn_cfg, color_cfg, false, true);

            // Botón [X] Cierre
            let x_btn_l = ancho - 26;
            let y_btn_t = 6;
            let x_btn_r = ancho - 8;
            let y_btn_b = 24;

            let (color_fondo_x, color_cruz) = if hover_cerrar {
                (0x001C2BC4, 0x00FFFFFF)
            } else {
                (color_fondo, color_texto_secundario)
            };

            if hover_cerrar {
                rellenar_rect(mem_dc, x_btn_l, y_btn_t, x_btn_r, y_btn_b, color_fondo_x);
            }

            let mut rect_btn_x = RECT {
                left: x_btn_l,
                top: y_btn_t,
                right: x_btn_r,
                bottom: y_btn_b,
            };
            dibujar_texto(mem_dc, "x", &mut rect_btn_x, color_cruz, false, true);
        }

        // Transferir a pantalla
        BitBlt(hdc, 0, 0, ancho, alto, mem_dc, 0, 0, SRCCOPY);

        // Limpieza de objetos GDI
        DeleteObject(font_encabezado);
        DeleteObject(font_etiquetas);
        DeleteObject(font_valores);
        DeleteObject(font_detalle);
        DeleteObject(font_flechas);
        SelectObject(mem_dc, bmp_anterior);
        DeleteObject(mem_bmp);
        DeleteDC(mem_dc);
    }

    y_sel_top_proc
}

/// Calcula la altura vertical óptima del panel según los módulos activos y la cantidad de discos.
fn calcular_altura_panel(config: &Configuracion, cantidad_discos: usize) -> i32 {
    let mut alto = 10; // Inicio sin barra fija reservada (máxima compacidad)

    if config.modulos.mostrar_cpu {
        alto += if config.modulos.mostrar_graficas {
            76
        } else {
            38
        } + 10;
    }
    if config.modulos.mostrar_ram {
        alto += 30;
    }
    if config.modulos.mostrar_gpu {
        alto += 30;
    }
    if config.modulos.mostrar_red {
        alto += 72;
    }
    if config.modulos.mostrar_discos {
        let n = cantidad_discos.max(1).min(4);
        alto += (n as i32) * 44 + 4;
    }
    if config.modulos.mostrar_top_procesos {
        alto += 76;
    }
    if config.modulos.mostrar_temperaturas {
        alto += 22;
    }

    alto + 14 // Padding inferior
}

/// Permite a la ventana de configuración notificar a la ventana principal los cambios.
pub fn notificar_actualizacion_configuracion(nueva_config: Configuracion) {
    if let Ok(mut lock) = ESTADO.lock() {
        if let Some(estado) = lock.as_mut() {
            let n_discos = estado.discos.unidades.len();
            let nuevo_alto = calcular_altura_panel(&nueva_config, n_discos);
            estado.config = nueva_config;
            estado.alto_panel = nuevo_alto;

            unsafe {
                let hwnd_h = estado.hwnd_principal as HWND;
                if hwnd_h != 0 as _ {
                    SetWindowPos(
                        hwnd_h,
                        0 as _,
                        0,
                        0,
                        estado.ancho_panel,
                        nuevo_alto,
                        SWP_NOZORDER | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE,
                    );

                    let rgn =
                        CreateRoundRectRgn(0, 0, estado.ancho_panel + 1, nuevo_alto + 1, 24, 24);
                    SetWindowRgn(hwnd_h, rgn, 1);
                    SetLayeredWindowAttributes(
                        hwnd_h,
                        0,
                        estado.config.ventana.opacidad,
                        LWA_ALPHA,
                    );
                    InvalidateRect(hwnd_h, ptr::null(), 0);
                }
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                SetTimer(hwnd, ID_TEMPORIZADOR, 1000, None);
                0
            }

            WM_NCHITTEST => {
                let mut pt = POINT {
                    x: (lparam as i32 & 0xFFFF) as i16 as i32,
                    y: ((lparam as i32 >> 16) & 0xFFFF) as i16 as i32,
                };
                ScreenToClient(hwnd, &mut pt);

                // Armar seguimiento de salida del cursor de la ventana
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);

                let (ancho, barra_visible) = if let Ok(lock) = ESTADO.lock() {
                    lock.as_ref()
                        .map(|s| (s.ancho_panel, s.barra_visible))
                        .unwrap_or((290, false))
                } else {
                    (290, false)
                };

                // Zona de activación superior delgada: borde superior de 0 a 6 píxeles de alto
                let en_zona_activacion = pt.x >= 0 && pt.x <= ancho && pt.y >= 0 && pt.y <= 6;
                // Zona interactiva de la barra superior: 0 a 28 píxeles
                let en_zona_barra = pt.x >= 0 && pt.x <= ancho && pt.y >= 0 && pt.y <= 28;

                let mut iniciar_timer_mostrar = false;
                let mut cancelar_timer_mostrar = false;
                let mut iniciar_timer_ocultar = false;
                let mut cancelar_timer_ocultar = false;
                let mut repintar = false;

                let sobre_x = barra_visible
                    && pt.x >= (ancho - 26)
                    && pt.x <= (ancho - 8)
                    && pt.y >= 6
                    && pt.y <= 26;
                let sobre_cfg = barra_visible
                    && pt.x >= (ancho - 48)
                    && pt.x <= (ancho - 30)
                    && pt.y >= 6
                    && pt.y <= 26;

                let sobre_selector = if let Ok(lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_ref() {
                        if estado.config.modulos.mostrar_top_procesos
                            && estado.y_selector_procesos > 0
                        {
                            pt.x >= (ancho - 105)
                                && pt.x <= (ancho - 16)
                                && pt.y >= estado.y_selector_procesos
                                && pt.y <= (estado.y_selector_procesos + 20)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if let Ok(mut lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_mut() {
                        if estado.hover_cerrar != sobre_x {
                            estado.hover_cerrar = sobre_x;
                            repintar = true;
                        }
                        if estado.hover_config != sobre_cfg {
                            estado.hover_config = sobre_cfg;
                            repintar = true;
                        }

                        if !estado.barra_visible {
                            if en_zona_activacion {
                                if estado.estado_barra == EstadoBarraSuperior::Oculta {
                                    estado.estado_barra = EstadoBarraSuperior::EsperandoAparicion;
                                    iniciar_timer_mostrar = true;
                                }
                            } else {
                                if estado.estado_barra == EstadoBarraSuperior::EsperandoAparicion {
                                    estado.estado_barra = EstadoBarraSuperior::Oculta;
                                    cancelar_timer_mostrar = true;
                                }
                            }
                        } else {
                            if en_zona_barra {
                                if estado.estado_barra == EstadoBarraSuperior::EsperandoOcultacion {
                                    estado.estado_barra = EstadoBarraSuperior::Visible;
                                    cancelar_timer_ocultar = true;
                                }
                            } else {
                                if estado.estado_barra == EstadoBarraSuperior::Visible
                                    && !crate::ui_config::esta_config_abierta()
                                {
                                    estado.estado_barra = EstadoBarraSuperior::EsperandoOcultacion;
                                    iniciar_timer_ocultar = true;
                                }
                            }
                        }
                    }
                }

                if iniciar_timer_mostrar {
                    SetTimer(hwnd, ID_TIMER_MOSTRAR_BARRA, 1000, None);
                }
                if cancelar_timer_mostrar {
                    KillTimer(hwnd, ID_TIMER_MOSTRAR_BARRA);
                }
                if iniciar_timer_ocultar {
                    SetTimer(hwnd, ID_TIMER_OCULTAR_BARRA, 600, None);
                }
                if cancelar_timer_ocultar {
                    KillTimer(hwnd, ID_TIMER_OCULTAR_BARRA);
                }
                if repintar {
                    InvalidateRect(hwnd, ptr::null(), 0);
                }

                if sobre_x || sobre_cfg || sobre_selector {
                    HTCLIENT as isize
                } else {
                    HTCAPTION as isize
                }
            }

            WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
                let mut cancelar_mostrar = false;
                let mut iniciar_ocultar = false;

                if let Ok(mut lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_mut() {
                        estado.hover_cerrar = false;
                        estado.hover_config = false;

                        if estado.estado_barra == EstadoBarraSuperior::EsperandoAparicion {
                            estado.estado_barra = EstadoBarraSuperior::Oculta;
                            cancelar_mostrar = true;
                        } else if estado.barra_visible
                            && estado.estado_barra == EstadoBarraSuperior::Visible
                        {
                            if !crate::ui_config::esta_config_abierta() {
                                estado.estado_barra = EstadoBarraSuperior::EsperandoOcultacion;
                                iniciar_ocultar = true;
                            }
                        }
                    }
                }

                if cancelar_mostrar {
                    KillTimer(hwnd, ID_TIMER_MOSTRAR_BARRA);
                }
                if iniciar_ocultar {
                    SetTimer(hwnd, ID_TIMER_OCULTAR_BARRA, 600, None);
                }
                InvalidateRect(hwnd, ptr::null(), 0);
                0
            }

            WM_LBUTTONUP => {
                let mut pt = POINT {
                    x: (lparam as i32 & 0xFFFF) as i16 as i32,
                    y: ((lparam as i32 >> 16) & 0xFFFF) as i16 as i32,
                };
                ScreenToClient(hwnd, &mut pt);

                let mut accion_cerrar = false;
                let mut accion_config = false;
                let mut config_copia = None;
                let mut clic_selector = false;

                if let Ok(mut lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_mut() {
                        if estado.barra_visible && estado.hover_cerrar {
                            accion_cerrar = true;
                        } else if estado.barra_visible && estado.hover_config {
                            accion_config = true;
                            config_copia = Some(estado.config.clone());
                        } else if estado.config.modulos.mostrar_top_procesos
                            && estado.y_selector_procesos > 0
                        {
                            let ancho = estado.ancho_panel;
                            if pt.x >= (ancho - 105)
                                && pt.x <= (ancho - 16)
                                && pt.y >= estado.y_selector_procesos
                                && pt.y <= (estado.y_selector_procesos + 20)
                            {
                                estado.modo_procesos = match estado.modo_procesos {
                                    ModoTopProcesos::Ram => ModoTopProcesos::Cpu,
                                    ModoTopProcesos::Cpu => ModoTopProcesos::Ram,
                                };
                                estado.top_procesos = estado
                                    .recolector_procesos
                                    .obtener_top_3(estado.modo_procesos);
                                clic_selector = true;
                            }
                        }
                    }
                }

                if accion_cerrar {
                    DestroyWindow(hwnd);
                } else if accion_config {
                    if let Some(cfg) = config_copia {
                        abrir_ventana_configuracion(cfg);
                    }
                } else if clic_selector {
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                0
            }

            WM_MOVE => {
                let x = (lparam as i32 & 0xFFFF) as i16 as i32;
                let y = ((lparam as i32 >> 16) & 0xFFFF) as i16 as i32;

                if let Ok(mut lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_mut() {
                        estado.posicion_actual = (x, y);
                    }
                }
                0
            }

            WM_DEVICECHANGE => {
                let mut cambio_tamano = false;
                let mut nuevo_alto = 0;
                let mut ancho = 0;

                if let Ok(mut lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_mut() {
                        let discos_nuevos = estado.recolector_discos.actualizar();
                        let num_ant = estado.discos.unidades.len();
                        let num_nuevos = discos_nuevos.unidades.len();
                        estado.discos = discos_nuevos;

                        if num_ant != num_nuevos {
                            nuevo_alto = calcular_altura_panel(&estado.config, num_nuevos);
                            if nuevo_alto != estado.alto_panel {
                                estado.alto_panel = nuevo_alto;
                                ancho = estado.ancho_panel;
                                cambio_tamano = true;
                            }
                        }
                    }
                }

                if cambio_tamano {
                    SetWindowPos(
                        hwnd,
                        0 as _,
                        0,
                        0,
                        ancho,
                        nuevo_alto,
                        SWP_NOZORDER | SWP_NOMOVE,
                    );
                    let rgn = CreateRoundRectRgn(0, 0, ancho + 1, nuevo_alto + 1, 24, 24);
                    SetWindowRgn(hwnd, rgn, 1);
                }

                InvalidateRect(hwnd, ptr::null(), 0);
                0
            }

            WM_TIMER => {
                if wparam == ID_TIMER_MOSTRAR_BARRA {
                    KillTimer(hwnd, ID_TIMER_MOSTRAR_BARRA);

                    let mut pt_screen = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt_screen);
                    let mut pt_client = pt_screen;
                    ScreenToClient(hwnd, &mut pt_client);

                    let mut activar = false;
                    if let Ok(mut lock) = ESTADO.lock() {
                        if let Some(estado) = lock.as_mut() {
                            if pt_client.x >= 0
                                && pt_client.x <= estado.ancho_panel
                                && pt_client.y >= 0
                                && pt_client.y <= 28
                            {
                                estado.barra_visible = true;
                                estado.estado_barra = EstadoBarraSuperior::Visible;
                                activar = true;
                            } else {
                                estado.estado_barra = EstadoBarraSuperior::Oculta;
                            }
                        }
                    }
                    if activar {
                        InvalidateRect(hwnd, ptr::null(), 0);
                    }
                } else if wparam == ID_TIMER_OCULTAR_BARRA {
                    KillTimer(hwnd, ID_TIMER_OCULTAR_BARRA);

                    if crate::ui_config::esta_config_abierta() {
                        return 0;
                    }

                    let mut pt_screen = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt_screen);
                    let mut pt_client = pt_screen;
                    ScreenToClient(hwnd, &mut pt_client);

                    let mut desactivar = false;
                    if let Ok(mut lock) = ESTADO.lock() {
                        if let Some(estado) = lock.as_mut() {
                            if pt_client.x >= 0
                                && pt_client.x <= estado.ancho_panel
                                && pt_client.y >= 0
                                && pt_client.y <= 28
                            {
                                estado.estado_barra = EstadoBarraSuperior::Visible;
                            } else {
                                estado.barra_visible = false;
                                estado.estado_barra = EstadoBarraSuperior::Oculta;
                                estado.hover_cerrar = false;
                                estado.hover_config = false;
                                desactivar = true;
                            }
                        }
                    }
                    if desactivar {
                        InvalidateRect(hwnd, ptr::null(), 0);
                    }
                } else if wparam == ID_TEMPORIZADOR {
                    let mut cambio_tamano = false;
                    let mut nuevo_alto = 0;
                    let mut ancho = 0;

                    if let Ok(mut lock) = ESTADO.lock() {
                        if let Some(estado) = lock.as_mut() {
                            // Métricas de CPU y RAM
                            let (cpu, ram, ghz) = estado.recolector_metricas.actualizar();
                            estado.uso_cpu = cpu;
                            estado.frecuencia_cpu_ghz = ghz;
                            estado.memoria = ram;

                            estado.historial_cpu.push(cpu);
                            if estado.historial_cpu.len() > MUESTRAS_HISTORIAL {
                                estado.historial_cpu.remove(0);
                            }

                            // Métricas de Red
                            estado.red = estado.recolector_red.actualizar();

                            // Métricas de GPU
                            estado.gpu = estado.recolector_gpu.actualizar();
                            estado.historial_gpu.push(estado.gpu.uso_porcentaje);
                            if estado.historial_gpu.len() > MUESTRAS_HISTORIAL {
                                estado.historial_gpu.remove(0);
                            }

                            // Métricas de Discos (detección dinámica periódica)
                            let discos_nuevos = estado.recolector_discos.actualizar();
                            let num_ant = estado.discos.unidades.len();
                            let num_nuevos = discos_nuevos.unidades.len();
                            estado.discos = discos_nuevos;

                            if num_ant != num_nuevos {
                                nuevo_alto = calcular_altura_panel(&estado.config, num_nuevos);
                                if nuevo_alto != estado.alto_panel {
                                    estado.alto_panel = nuevo_alto;
                                    ancho = estado.ancho_panel;
                                    cambio_tamano = true;
                                }
                            }

                            // Métricas de Top Procesos (actualización cada 2 segundos)
                            if estado.config.modulos.mostrar_top_procesos {
                                estado.tick_procesos += 1;
                                if estado.tick_procesos >= 2 {
                                    estado.top_procesos = estado
                                        .recolector_procesos
                                        .obtener_top_3(estado.modo_procesos);
                                    estado.tick_procesos = 0;
                                }
                            }

                            // Métricas de Temperatura
                            estado.temperaturas = estado
                                .recolector_temperaturas
                                .actualizar(estado.gpu.temperatura_celsius);
                        }
                    }

                    if cambio_tamano {
                        SetWindowPos(
                            hwnd,
                            0 as _,
                            0,
                            0,
                            ancho,
                            nuevo_alto,
                            SWP_NOZORDER | SWP_NOMOVE,
                        );
                        let rgn = CreateRoundRectRgn(0, 0, ancho + 1, nuevo_alto + 1, 24, 24);
                        SetWindowRgn(hwnd, rgn, 1);
                    }

                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                0
            }

            WM_PAINT => {
                let mut ps: PAINTSTRUCT = mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);

                let datos = if let Ok(lock) = ESTADO.lock() {
                    lock.as_ref().map(|s| {
                        (
                            s.ancho_panel,
                            s.alto_panel,
                            s.uso_cpu,
                            s.frecuencia_cpu_ghz,
                            s.historial_cpu.clone(),
                            s.memoria,
                            s.red.clone(),
                            s.gpu.clone(),
                            s.historial_gpu.clone(),
                            s.discos.clone(),
                            s.top_procesos.clone(),
                            s.modo_procesos,
                            s.temperaturas,
                            s.hover_cerrar,
                            s.hover_config,
                            s.barra_visible,
                            s.config.clone(),
                        )
                    })
                } else {
                    None
                };

                if let Some((
                    ancho,
                    alto,
                    cpu,
                    ghz,
                    h_cpu,
                    ram,
                    red,
                    gpu,
                    h_gpu,
                    discos,
                    top_proc,
                    modo_proc,
                    temps,
                    h_cerrar,
                    h_cfg,
                    barra_vis,
                    cfg,
                )) = datos
                {
                    let y_sel = pintar_panel(
                        hdc, ancho, alto, cpu, ghz, &h_cpu, ram, &red, &gpu, &h_gpu, &discos,
                        &top_proc, modo_proc, &temps, h_cerrar, h_cfg, barra_vis, &cfg,
                    );
                    if let Ok(mut lock) = ESTADO.lock() {
                        if let Some(estado) = lock.as_mut() {
                            estado.y_selector_procesos = y_sel;
                        }
                    }
                }

                EndPaint(hwnd, &ps);
                0
            }

            WM_CLOSE => {
                println!("[Nexus] Solicitud de cierre recibida.");
                DestroyWindow(hwnd);
                0
            }

            WM_DESTROY => {
                println!("[Nexus] Guardando última posición en disco...");
                if let Ok(lock) = ESTADO.lock() {
                    if let Some(estado) = lock.as_ref() {
                        let (pos_x, pos_y) = estado.posicion_actual;
                        guardar_posicion_en_disco(pos_x, pos_y);
                    }
                }

                KillTimer(hwnd, ID_TEMPORIZADOR);
                KillTimer(hwnd, ID_TIMER_MOSTRAR_BARRA);
                KillTimer(hwnd, ID_TIMER_OCULTAR_BARRA);
                PostQuitMessage(0);
                0
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() {
    // Instancia única (Single Instance): evitar abrir múltiples copias concurrentes de Nexus
    let _mutex = unsafe {
        let nombre_mutex = a_wstring("Local\\NexusMonitorSingleInstanceMutex");
        let mutex = CreateMutexW(ptr::null(), 1, nombre_mutex.as_ptr());
        let err = GetLastError();
        if mutex != 0 as _ && err == ERROR_ALREADY_EXISTS {
            let nombre_clase = a_wstring("NexusMonitorWindowV4");
            let hwnd_existente = FindWindowW(nombre_clase.as_ptr(), ptr::null());
            if hwnd_existente != 0 as _ {
                SetForegroundWindow(hwnd_existente);
            }
            return;
        }
        mutex
    };

    let config = cargar_o_crear_por_defecto();

    if config.comportamiento.iniciar_con_windows {
        autostart::configurar_inicio_automatico(true);
    }

    let mut recolector_metricas = RecolectorMetricas::nuevo();
    let (cpu_inicial, ram_inicial, ghz_inicial) = recolector_metricas.actualizar();

    let mut recolector_red = RecolectorRed::nuevo();
    let red_inicial = recolector_red.actualizar();

    let mut recolector_gpu = RecolectorGPU::nuevo();
    let gpu_inicial = recolector_gpu.actualizar();

    let mut recolector_discos = RecolectorDiscos::nuevo();
    let discos_inicial = recolector_discos.actualizar();

    let mut recolector_temperaturas = RecolectorTemperaturas::nuevo();
    let temps_inicial = recolector_temperaturas.actualizar(gpu_inicial.temperatura_celsius);

    let modo_procesos_inicial = if config.modulos.modo_top_procesos.to_lowercase() == "cpu" {
        ModoTopProcesos::Cpu
    } else {
        ModoTopProcesos::Ram
    };
    let mut recolector_procesos = RecolectorProcesos::nuevo();
    let top_procesos_inicial = if config.modulos.mostrar_top_procesos {
        recolector_procesos.obtener_top_3(modo_procesos_inicial)
    } else {
        Vec::new()
    };

    let mut historial_cpu = Vec::with_capacity(MUESTRAS_HISTORIAL);
    historial_cpu.push(cpu_inicial);

    let mut historial_gpu = Vec::with_capacity(MUESTRAS_HISTORIAL);
    historial_gpu.push(gpu_inicial.uso_porcentaje);

    const ANCHO_PANEL: i32 = 290;
    let alto_panel = calcular_altura_panel(&config, discos_inicial.unidades.len());

    let pantalla_ancho = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let pos_x = if config.ventana.posicion_x < 0 {
        pantalla_ancho - ANCHO_PANEL - 25
    } else {
        config.ventana.posicion_x
    };
    let pos_y = config.ventana.posicion_y.max(10);

    unsafe {
        let nombre_clase = a_wstring("NexusMonitorWindowV4");
        let titulo_ventana = a_wstring("Nexus Monitor");

        let clase_ventana = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: 0 as _,
            hIcon: 0 as _,
            hCursor: LoadCursorW(0 as _, IDC_ARROW),
            hbrBackground: 0 as _,
            lpszMenuName: ptr::null(),
            lpszClassName: nombre_clase.as_ptr(),
        };

        let atom = RegisterClassW(&clase_ventana);
        let err_reg = GetLastError();
        if atom == 0 && err_reg != 1410 {
            return;
        }

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED,
            nombre_clase.as_ptr(),
            titulo_ventana.as_ptr(),
            WS_POPUP | WS_VISIBLE | WS_SYSMENU,
            pos_x,
            pos_y,
            ANCHO_PANEL,
            alto_panel,
            0 as _,
            0 as _,
            0 as _,
            ptr::null(),
        );

        if hwnd == 0 as _ {
            return;
        }

        if let Ok(mut lock) = ESTADO.lock() {
            *lock = Some(EstadoApp {
                recolector_metricas,
                recolector_red,
                recolector_gpu,
                recolector_discos,
                recolector_temperaturas,
                recolector_procesos,
                config: config.clone(),
                uso_cpu: cpu_inicial,
                frecuencia_cpu_ghz: ghz_inicial,
                historial_cpu,
                memoria: ram_inicial,
                red: red_inicial,
                gpu: gpu_inicial,
                historial_gpu,
                discos: discos_inicial.clone(),
                top_procesos: top_procesos_inicial,
                modo_procesos: modo_procesos_inicial,
                tick_procesos: 0,
                y_selector_procesos: 0,
                temperaturas: temps_inicial,
                estado_barra: EstadoBarraSuperior::Oculta,
                barra_visible: false,
                hover_cerrar: false,
                hover_config: false,
                posicion_actual: (pos_x, pos_y),
                ancho_panel: ANCHO_PANEL,
                alto_panel,
                hwnd_principal: hwnd as isize,
            });
        }

        let rgn = CreateRoundRectRgn(0, 0, ANCHO_PANEL + 1, alto_panel + 1, 24, 24);
        SetWindowRgn(hwnd, rgn, 1);
        SetLayeredWindowAttributes(hwnd, 0, config.ventana.opacidad, LWA_ALPHA);

        ShowWindow(hwnd, SW_SHOW);

        println!(
            "[Nexus] Monitor v0.4.0 iniciado con éxito en (X={}, Y={}).",
            pos_x, pos_y
        );
        println!("  - Módulos activos: CPU, RAM, GPU, Red, Discos, Top Procesos y Temperaturas.");
        for disco in &discos_inicial.unidades {
            println!(
                "  - Unidad detectada: {} ({}) — {} | {:.0}/{:.0} GB ({:.0}%)",
                disco.letra,
                if disco.etiqueta.is_empty() {
                    "sin etiqueta"
                } else {
                    &disco.etiqueta
                },
                disco.tipo_medio,
                disco.usado_gb,
                disco.total_gb,
                disco.porcentaje_uso,
            );
        }
        println!(
            "  - Barra superior inteligente: Permanece oculta hasta posar el cursor 1 segundo en el borde superior."
        );
        println!(
            "  - Inicio automático: Registrado en HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run."
        );
        println!("  - Cierre: Botón [x] o Alt + F4.\n");

        let mut msg: MSG = mem::zeroed();
        loop {
            let res = GetMessageW(&mut msg, 0 as _, 0, 0);
            if res <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        println!("[Nexus] Monitor finalizado.");
    }
}
