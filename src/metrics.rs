//! Módulo de lectura de métricas de CPU y RAM para Windows.

use std::mem;
use std::time::Instant;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE,
    PDH_FMT_DOUBLE,
};
use windows_sys::Win32::System::SystemInformation::{
    GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// Información procesada de memoria RAM.
#[derive(Debug, Clone, Copy)]
pub struct MemoriaInfo {
    pub total_gb: f64,
    pub usada_gb: f64,
    pub porcentaje_uso: f64,
}

/// Convierte la estructura FILETIME de Windows a un entero de 64 bits.
pub fn filetime_a_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

/// Consulta el estado actual de la memoria RAM.
pub fn obtener_memoria_ram() -> Option<MemoriaInfo> {
    unsafe {
        let mut estado: MEMORYSTATUSEX = mem::zeroed();
        estado.dwLength = mem::size_of::<MEMORYSTATUSEX>() as u32;

        if GlobalMemoryStatusEx(&mut estado) == 0 {
            return None;
        }

        const BYTES_POR_GB: f64 = 1024.0 * 1024.0 * 1024.0;
        let total_bytes = estado.ullTotalPhys;
        let disponible_bytes = estado.ullAvailPhys;
        let usada_bytes = total_bytes.saturating_sub(disponible_bytes);

        let total_gb = total_bytes as f64 / BYTES_POR_GB;
        let usada_gb = usada_bytes as f64 / BYTES_POR_GB;
        let porcentaje_uso = if total_bytes > 0 {
            (usada_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        Some(MemoriaInfo {
            total_gb,
            usada_gb,
            porcentaje_uso,
        })
    }
}

/// Consulta los contadores de tiempo del sistema (Kernel, Usuario e Idle).
pub fn obtener_tiempos_cpu() -> Option<(u64, u64, u64)> {
    unsafe {
        let mut idle = mem::zeroed::<FILETIME>();
        let mut kernel = mem::zeroed::<FILETIME>();
        let mut user = mem::zeroed::<FILETIME>();

        if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
            return None;
        }

        Some((
            filetime_a_u64(idle),
            filetime_a_u64(kernel),
            filetime_a_u64(user),
        ))
    }
}

/// Calcula el porcentaje de uso de CPU a partir de dos muestras consecutivas.
pub fn calcular_porcentaje_cpu(
    prev: (u64, u64, u64),
    actual: (u64, u64, u64),
) -> f64 {
    let (idle_prev, kernel_prev, user_prev) = prev;
    let (idle_act, kernel_act, user_act) = actual;

    let delta_idle = idle_act.saturating_sub(idle_prev);
    let delta_kernel = kernel_act.saturating_sub(kernel_prev);
    let delta_user = user_act.saturating_sub(user_prev);

    let delta_total = delta_kernel + delta_user;
    if delta_total == 0 {
        return 0.0;
    }

    let delta_trabajado = delta_total.saturating_sub(delta_idle);
    (delta_trabajado as f64 / delta_total as f64) * 100.0
}

/// Recolector de métricas de CPU y RAM con soporte para frecuencia dinámica.
pub struct RecolectorMetricas {
    tiempos_cpu_prev: Option<(u64, u64, u64)>,
    query_pdh: Option<isize>,
    counter_frecuencia: Option<isize>,
    ultima_frecuencia_ghz: Option<f64>,
    ultimo_tiempo_pdh: Instant,
}

impl RecolectorMetricas {
    pub fn nuevo() -> Self {
        let mut recolector = Self {
            tiempos_cpu_prev: obtener_tiempos_cpu(),
            query_pdh: None,
            counter_frecuencia: None,
            ultima_frecuencia_ghz: None,
            ultimo_tiempo_pdh: Instant::now(),
        };

        recolector.inicializar_pdh();
        recolector
    }

    fn inicializar_pdh(&mut self) {
        unsafe {
            let mut query: isize = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0 {
                let mut counter: isize = 0;
                // Contador en inglés estándar para la frecuencia de CPU
                let ruta_counter = crate::a_wstring("\\Processor Information(_Total)\\Processor Frequency");
                if PdhAddEnglishCounterW(query, ruta_counter.as_ptr(), 0, &mut counter) == 0 {
                    let _ = PdhCollectQueryData(query);
                    self.query_pdh = Some(query);
                    self.counter_frecuencia = Some(counter);
                } else {
                    let _ = PdhCloseQuery(query);
                }
            }
        }
    }

    /// Actualiza y devuelve el porcentaje de CPU actual, memoria RAM y la frecuencia estimada.
    pub fn actualizar(&mut self) -> (f64, Option<MemoriaInfo>, Option<f64>) {
        let tiempos_actual = obtener_tiempos_cpu();
        let porcentaje_cpu = match (self.tiempos_cpu_prev, tiempos_actual) {
            (Some(prev), Some(act)) => calcular_porcentaje_cpu(prev, act),
            _ => 0.0,
        };

        if tiempos_actual.is_some() {
            self.tiempos_cpu_prev = tiempos_actual;
        }

        let memoria = obtener_memoria_ram();

        // Consulta de frecuencia de CPU mediante PDH (si está disponible)
        let ahora = Instant::now();
        if ahora.duration_since(self.ultimo_tiempo_pdh).as_millis() >= 900 {
            self.ultimo_tiempo_pdh = ahora;
            if let (Some(query), Some(counter)) = (self.query_pdh, self.counter_frecuencia) {
                unsafe {
                    if PdhCollectQueryData(query) == 0 {
                        let mut valor: PDH_FMT_COUNTERVALUE = mem::zeroed();
                        if PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, std::ptr::null_mut(), &mut valor) == 0 {
                            let mhz = valor.Anonymous.doubleValue;
                            if mhz > 100.0 {
                                self.ultima_frecuencia_ghz = Some(mhz / 1000.0);
                            }
                        }
                    }
                }
            }
        }

        (porcentaje_cpu, memoria, self.ultima_frecuencia_ghz)
    }
}

impl Drop for RecolectorMetricas {
    fn drop(&mut self) {
        if let Some(query) = self.query_pdh.take() {
            unsafe {
                let _ = PdhCloseQuery(query);
            }
        }
    }
}
