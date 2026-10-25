//! Módulo de supervisión de discos y almacenamiento para Windows.
//!
//! Supervisa todas las unidades lógicas conectadas al sistema (discos internos,
//! SSDs, discos duros externos USB y pendrives), identificando letra, etiqueta de volumen,
//! tipo de medio (SSD, HDD, USB) y espacio usado/total.

use std::mem;
use std::ptr;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetLogicalDriveStringsW, GetVolumeInformationW, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};

/// Información detallada de una unidad de almacenamiento accesible.
#[derive(Debug, Clone, PartialEq)]
pub struct InfoDisco {
    pub letra: String,
    pub etiqueta: String,
    pub tipo_medio: String,
    pub total_gb: f64,
    pub usado_gb: f64,
    pub libre_gb: f64,
    pub porcentaje_uso: f64,
}

/// Estado global de almacenamiento y tasas de transferencia.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EstadoDiscos {
    pub unidades: Vec<InfoDisco>,
    pub velocidad_lectura_bps: f64,
    pub velocidad_escritura_bps: f64,
}

pub struct RecolectorDiscos {
    query_pdh: Option<isize>,
    counter_read: Option<isize>,
    counter_write: Option<isize>,
}

impl RecolectorDiscos {
    pub fn nuevo() -> Self {
        let mut query: isize = 0;
        let mut counter_read = None;
        let mut counter_write = None;
        let mut query_pdh = None;

        unsafe {
            if PdhOpenQueryW(ptr::null(), 0, &mut query) == 0 {
                let mut c_read: isize = 0;
                let mut c_write: isize = 0;

                let r_read = crate::a_wstring("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec");
                let r_write = crate::a_wstring("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec");

                let ok_r = PdhAddEnglishCounterW(query, r_read.as_ptr(), 0, &mut c_read) == 0;
                let ok_w = PdhAddEnglishCounterW(query, r_write.as_ptr(), 0, &mut c_write) == 0;

                if ok_r && ok_w {
                    let _ = PdhCollectQueryData(query);
                    query_pdh = Some(query);
                    counter_read = Some(c_read);
                    counter_write = Some(c_write);
                } else {
                    let _ = PdhCloseQuery(query);
                }
            }
        }

        Self {
            query_pdh,
            counter_read,
            counter_write,
        }
    }

    /// Lee todas las unidades de almacenamiento accesibles (fijas y extraíbles) y su espacio.
    pub fn actualizar(&mut self) -> EstadoDiscos {
        let mut unidades = Vec::new();

        unsafe {
            let mut buffer = [0u16; 512];
            let len = GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr());

            if len > 0 {
                let mut start = 0;
                for i in 0..len as usize {
                    if buffer[i] == 0 {
                        if start < i {
                            let drive_slice = &buffer[start..=i];
                            let tipo_drive = GetDriveTypeW(drive_slice.as_ptr());

                            // 2 = DRIVE_REMOVABLE (Pendrives, memorias USB)
                            // 3 = DRIVE_FIXED (Discos internos SSD/HDD y discos externos USB)
                            if tipo_drive == 2 || tipo_drive == 3 {
                                let letra_completa = String::from_utf16_lossy(&buffer[start..i]);
                                let letra_corta = letra_completa.trim_end_matches('\\').to_string();

                                let mut free_avail = 0u64;
                                let mut total_bytes = 0u64;
                                let mut free_total = 0u64;

                                // Consulta de espacio (GetDiskFreeSpaceExW)
                                let ok_espacio = GetDiskFreeSpaceExW(
                                    drive_slice.as_ptr(),
                                    &mut free_avail,
                                    &mut total_bytes,
                                    &mut free_total,
                                );

                                if ok_espacio != 0 && total_bytes > 0 {
                                    const BYTES_POR_GB: f64 = 1024.0 * 1024.0 * 1024.0;
                                    let total_gb = total_bytes as f64 / BYTES_POR_GB;
                                    let libre_gb = free_avail as f64 / BYTES_POR_GB;
                                    let usado_gb = (total_gb - libre_gb).max(0.0);
                                    let pct = if total_gb > 0.0 {
                                        (usado_gb / total_gb) * 100.0
                                    } else {
                                        0.0
                                    };

                                    // Etiqueta del volumen
                                    let mut vol_name = [0u16; 261];
                                    let ok_vol = GetVolumeInformationW(
                                        drive_slice.as_ptr(),
                                        vol_name.as_mut_ptr(),
                                        vol_name.len() as u32,
                                        ptr::null_mut(),
                                        ptr::null_mut(),
                                        ptr::null_mut(),
                                        ptr::null_mut(),
                                        0,
                                    );
                                    let etiqueta = if ok_vol != 0 {
                                        let pos_null =
                                            vol_name.iter().position(|&c| c == 0).unwrap_or(0);
                                        String::from_utf16_lossy(&vol_name[..pos_null])
                                            .trim()
                                            .to_string()
                                    } else {
                                        String::new()
                                    };

                                    // Tipo de medio (SSD, HDD, USB)
                                    let tipo_medio =
                                        determinar_tipo_medio(&letra_corta, tipo_drive);

                                    unidades.push(InfoDisco {
                                        letra: letra_corta,
                                        etiqueta,
                                        tipo_medio,
                                        total_gb,
                                        usado_gb,
                                        libre_gb,
                                        porcentaje_uso: pct,
                                    });
                                }
                            }
                        }
                        start = i + 1;
                    }
                }
            }
        }

        // Priorizar la unidad del sistema (C:) al inicio, y ordenar el resto alfabéticamente
        unidades.sort_by(|a, b| {
            let a_es_c = a.letra.starts_with('C') || a.letra.starts_with('c');
            let b_es_c = b.letra.starts_with('C') || b.letra.starts_with('c');
            match (a_es_c, b_es_c) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.letra.cmp(&b.letra),
            }
        });

        // Métricas de actividad de lectura/escritura mediante PDH
        let mut vel_r = 0.0;
        let mut vel_w = 0.0;

        if let (Some(query), Some(c_r), Some(c_w)) =
            (self.query_pdh, self.counter_read, self.counter_write)
        {
            unsafe {
                if PdhCollectQueryData(query) == 0 {
                    let mut val_r: PDH_FMT_COUNTERVALUE = mem::zeroed();
                    let mut val_w: PDH_FMT_COUNTERVALUE = mem::zeroed();

                    if PdhGetFormattedCounterValue(c_r, PDH_FMT_DOUBLE, ptr::null_mut(), &mut val_r)
                        == 0
                    {
                        vel_r = val_r.Anonymous.doubleValue.max(0.0);
                    }
                    if PdhGetFormattedCounterValue(c_w, PDH_FMT_DOUBLE, ptr::null_mut(), &mut val_w)
                        == 0
                    {
                        vel_w = val_w.Anonymous.doubleValue.max(0.0);
                    }
                }
            }
        }

        EstadoDiscos {
            unidades,
            velocidad_lectura_bps: vel_r,
            velocidad_escritura_bps: vel_w,
        }
    }
}

/// Determina si una unidad es SSD, HDD o USB utilizando DeviceIoControl de Windows.
fn determinar_tipo_medio(letra: &str, tipo_drive: u32) -> String {
    // Si Windows lo cataloga directamente como DRIVE_REMOVABLE (2), es un pendrive/USB
    if tipo_drive == 2 {
        return "USB".to_string();
    }

    // Para DRIVE_FIXED (3), consultamos el bus de almacenamiento (USB vs interno) y el seek penalty
    let path_dispositivo = format!("\\\\.\\{}", letra.trim_end_matches('\\'));
    let wpath = crate::a_wstring(&path_dispositivo);

    unsafe {
        // Abrir el volumen sin requerir permisos de administrador (dwDesiredAccess = 0)
        let handle = CreateFileW(
            wpath.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            0 as _,
        );

        if handle != 0 as _ && handle != -1 as _ {
            // 1. Consultar tipo de bus (StorageDeviceProperty = 0)
            #[repr(C)]
            struct StoragePropertyQuery {
                property_id: u32,
                query_type: u32,
                additional_parameters: [u8; 1],
            }

            let query_bus = StoragePropertyQuery {
                property_id: 0, // StorageDeviceProperty
                query_type: 0,  // PropertyStandardQuery
                additional_parameters: [0],
            };

            let mut out_buf = [0u8; 1024];
            let mut bytes_ret = 0u32;
            let ok_bus = DeviceIoControl(
                handle,
                0x002D1400, // IOCTL_STORAGE_QUERY_PROPERTY
                &query_bus as *const _ as *const _,
                mem::size_of::<StoragePropertyQuery>() as u32,
                out_buf.as_mut_ptr() as *mut _,
                out_buf.len() as u32,
                &mut bytes_ret,
                ptr::null_mut(),
            );

            if ok_bus != 0 && bytes_ret >= 32 {
                // BusType se ubica en el offset 28 como u32 little-endian
                let bus_type =
                    u32::from_ne_bytes([out_buf[28], out_buf[29], out_buf[30], out_buf[31]]);

                // BusTypeUsb = 0x07
                if bus_type == 7 {
                    CloseHandle(handle);
                    return "USB".to_string();
                }
            }

            // 2. Si no es USB, consultar penalización de búsqueda para diferenciar SSD vs HDD
            let query_seek = StoragePropertyQuery {
                property_id: 7, // StorageDeviceSeekPenaltyProperty
                query_type: 0,  // PropertyStandardQuery
                additional_parameters: [0],
            };

            let mut seek_buf = [0u8; 32];
            let mut ret_seek = 0u32;
            let ok_seek = DeviceIoControl(
                handle,
                0x002D1400, // IOCTL_STORAGE_QUERY_PROPERTY
                &query_seek as *const _ as *const _,
                mem::size_of::<StoragePropertyQuery>() as u32,
                seek_buf.as_mut_ptr() as *mut _,
                seek_buf.len() as u32,
                &mut ret_seek,
                ptr::null_mut(),
            );

            CloseHandle(handle);

            if ok_seek != 0 && ret_seek >= 9 {
                // IncursSeekPenalty en offset 8 (0 = SSD, 1 = HDD)
                let incurs_seek = seek_buf[8];
                if incurs_seek == 0 {
                    return "SSD".to_string();
                } else {
                    return "HDD".to_string();
                }
            }
        }
    }

    // Heurística de respaldo en caso de que DeviceIoControl no responda:
    if letra.starts_with('C') || letra.starts_with('c') {
        "SSD".to_string()
    } else {
        "HDD".to_string()
    }
}

impl Drop for RecolectorDiscos {
    fn drop(&mut self) {
        if let Some(query) = self.query_pdh.take() {
            unsafe {
                let _ = PdhCloseQuery(query);
            }
        }
    }
}
