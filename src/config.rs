//! Módulo de configuración persistente para Nexus Monitor v0.4.

use std::fs;

use serde::{Deserialize, Serialize};

const ARCHIVO_CONFIG: &str = "nexus.toml";

/// Configuración global de la aplicación.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Configuracion {
    pub ventana: ConfigVentana,
    pub modulos: ConfigModulos,
    pub colores: ConfigColores,
    pub tipografia: ConfigTipografia,
    pub comportamiento: ConfigComportamiento,
}

/// Opciones de la ventana en el escritorio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigVentana {
    pub posicion_x: i32,
    pub posicion_y: i32,
    pub opacidad: u8, // Valor entre 0 y 255 (ej: 230 = ~90% de opacidad)
}

fn default_mostrar_top_procesos() -> bool {
    true
}

fn default_modo_top_procesos() -> String {
    "ram".to_string()
}

/// Módulos que se desean mostrar u ocultar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigModulos {
    pub mostrar_cpu: bool,
    pub mostrar_ram: bool,
    pub mostrar_red: bool,
    pub mostrar_gpu: bool,
    pub mostrar_discos: bool,
    pub mostrar_temperaturas: bool,
    pub mostrar_graficas: bool,
    #[serde(default = "default_mostrar_top_procesos")]
    pub mostrar_top_procesos: bool,
    #[serde(default = "default_modo_top_procesos")]
    pub modo_top_procesos: String,
}

/// Paleta de colores visual para el panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigColores {
    pub fondo: String,             // Color de fondo del panel (ej: "#14151C")
    pub fondo_tarjeta: String,     // Fondo de tarjetas internas (ej: "#1E202A")
    pub borde: String,             // Borde perimetral fino (ej: "#2B2E3C")
    pub acento_cpu: String,        // Color de CPU (ej: "#38BDF8")
    pub acento_cpu_alto: String,   // Color si la CPU supera el 75% (ej: "#FB923C")
    pub acento_ram: String,        // Color de RAM (ej: "#2DD4BF")
    pub acento_ram_alto: String,   // Color si la RAM supera el 85% (ej: "#F43F5E")
    pub acento_descarga: String,   // Flecha de descarga (ej: "#2DD4BF")
    pub acento_subida: String,     // Flecha de subida (ej: "#FB923C")
    pub acento_gpu: String,        // Color de GPU (ej: "#C084FC" violeta)
    pub acento_disco: String,      // Color de Disco (ej: "#FACC15" ámbar)
    pub texto_principal: String,  // Texto de etiquetas principales (ej: "#FFFFFF")
    pub texto_secundario: String, // Texto de subtítulos y detalles (ej: "#94A3B8")
}

/// Opciones de fuente tipográfica.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigTipografia {
    pub fuente: String,
    pub tamano_base: i32,
}

fn default_iniciar_con_windows() -> bool {
    true
}

/// Opciones de comportamiento del monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigComportamiento {
    pub longitud_historial: usize,
    pub intervalo_actualizacion_ms: u32,
    #[serde(default = "default_iniciar_con_windows")]
    pub iniciar_con_windows: bool,
}

impl Default for Configuracion {
    fn default() -> Self {
        Self {
            ventana: ConfigVentana {
                posicion_x: -1,
                posicion_y: 35,
                opacidad: 230,
            },
            modulos: ConfigModulos {
                mostrar_cpu: true,
                mostrar_ram: true,
                mostrar_red: true,
                mostrar_gpu: true,
                mostrar_discos: true,
                mostrar_temperaturas: true,
                mostrar_graficas: true,
                mostrar_top_procesos: true,
                modo_top_procesos: "ram".to_string(),
            },
            colores: ConfigColores {
                fondo: "#14151C".to_string(),
                fondo_tarjeta: "#1E202A".to_string(),
                borde: "#2B2E3C".to_string(),
                acento_cpu: "#38BDF8".to_string(),
                acento_cpu_alto: "#FB923C".to_string(),
                acento_ram: "#2DD4BF".to_string(),
                acento_ram_alto: "#F43F5E".to_string(),
                acento_descarga: "#2DD4BF".to_string(),
                acento_subida: "#FB923C".to_string(),
                acento_gpu: "#C084FC".to_string(),
                acento_disco: "#FACC15".to_string(),
                texto_principal: "#FFFFFF".to_string(),
                texto_secundario: "#94A3B8".to_string(),
            },
            tipografia: ConfigTipografia {
                fuente: "Segoe UI".to_string(),
                tamano_base: 13,
            },
            comportamiento: ConfigComportamiento {
                longitud_historial: 28,
                intervalo_actualizacion_ms: 1000,
                iniciar_con_windows: true,
            },
        }
    }
}

/// Devuelve la ruta absoluta al archivo `nexus.toml` situado junto al ejecutable.
pub fn ruta_archivo_config() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join(ARCHIVO_CONFIG);
        }
    }
    std::path::PathBuf::from(ARCHIVO_CONFIG)
}

/// Carga la configuración desde `nexus.toml`. Si faltan campos o no existe,
/// genera uno nuevo preservando compatibilidad.
pub fn cargar_o_crear_por_defecto() -> Configuracion {
    let ruta = ruta_archivo_config();
    if ruta.exists() {
        if let Ok(contenido) = fs::read_to_string(&ruta) {
            if let Ok(config) = toml::from_str::<Configuracion>(&contenido) {
                return config;
            }
        }
    }

    let config_defecto = Configuracion::default();
    if let Ok(toml_str) = toml::to_string_pretty(&config_defecto) {
        let _ = fs::write(&ruta, toml_str);
    }
    config_defecto
}

/// Guarda la configuración completa en disco.
pub fn guardar_configuracion(config: &Configuracion) {
    let ruta = ruta_archivo_config();
    if let Ok(toml_str) = toml::to_string_pretty(config) {
        let _ = fs::write(&ruta, toml_str);
    }
}

/// Guarda en el archivo de configuración la nueva posición de la ventana.
pub fn guardar_posicion_en_disco(pos_x: i32, pos_y: i32) {
    let mut config = cargar_o_crear_por_defecto();
    config.ventana.posicion_x = pos_x;
    config.ventana.posicion_y = pos_y;
    guardar_configuracion(&config);
}

/// Convierte una cadena de color hexadecimal tipo "#RRGGBB" al formato
/// de color nativo de Windows GDI (`COLORREF`: `0x00BBGGRR`).
pub fn hex_a_bgr(hex: &str) -> u32 {
    let limpio = hex.trim_start_matches('#');
    if limpio.len() == 6 {
        if let Ok(num) = u32::from_str_radix(limpio, 16) {
            let r = (num >> 16) & 0xFF;
            let g = (num >> 8) & 0xFF;
            let b = num & 0xFF;
            return (b << 16) | (g << 8) | r;
        }
    }
    0x00FFFFFF
}
