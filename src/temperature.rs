//! Módulo de lectura de sensores térmicos nativos en Windows.
//!
//! Lee la temperatura de la zona térmica ACPI del sistema mediante PDH.
//! Si el sensor no está disponible o no responde, reporta `None` de forma honesta.

use std::mem;

use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};

/// Temperaturas recopiladas de los componentes.
#[derive(Debug, Clone, Copy)]
pub struct InfoTemperaturas {
    pub cpu_celsius: Option<f64>,
    pub gpu_celsius: Option<f64>,
}

pub struct RecolectorTemperaturas {
    query_pdh: Option<isize>,
    counter_tz: Option<isize>,
}

impl RecolectorTemperaturas {
    pub fn nuevo() -> Self {
        let mut query: isize = 0;
        let mut counter_tz = None;
        let mut query_pdh = None;

        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0 {
                let mut c: isize = 0;
                let ruta = crate::a_wstring("\\Thermal Zone Information(*)\\Temperature");

                if PdhAddEnglishCounterW(query, ruta.as_ptr(), 0, &mut c) == 0 {
                    let _ = PdhCollectQueryData(query);
                    query_pdh = Some(query);
                    counter_tz = Some(c);
                } else {
                    let _ = PdhCloseQuery(query);
                }
            }
        }

        Self {
            query_pdh,
            counter_tz,
        }
    }

    /// Lee los sensores térmicos disponibles.
    pub fn actualizar(&mut self, temp_gpu_estimada: Option<f64>) -> InfoTemperaturas {
        let mut cpu_temp = None;

        if let (Some(query), Some(c)) = (self.query_pdh, self.counter_tz) {
            unsafe {
                if PdhCollectQueryData(query) == 0 {
                    let mut val: PDH_FMT_COUNTERVALUE = mem::zeroed();
                    if PdhGetFormattedCounterValue(
                        c,
                        PDH_FMT_DOUBLE,
                        std::ptr::null_mut(),
                        &mut val,
                    ) == 0
                    {
                        let kelvin = val.Anonymous.doubleValue;
                        if kelvin > 200.0 && kelvin < 450.0 {
                            cpu_temp = Some(kelvin - 273.15);
                        }
                    }
                }
            }
        }

        InfoTemperaturas {
            cpu_celsius: cpu_temp,
            gpu_celsius: temp_gpu_estimada,
        }
    }
}

impl Drop for RecolectorTemperaturas {
    fn drop(&mut self) {
        if let Some(query) = self.query_pdh.take() {
            unsafe {
                let _ = PdhCloseQuery(query);
            }
        }
    }
}
