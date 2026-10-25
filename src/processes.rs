//! Módulo de supervisión de los procesos que más recursos consumen (Top Procesos)
//! para Windows.
//!
//! Utiliza la API nativa de alto rendimiento `NtQuerySystemInformation` de `ntdll.dll`
//! para consultar de forma atómica y ligera todos los procesos del sistema sin necesidad
//! de abrir handles (`OpenProcess`) ni privilegios de administrador.

use std::collections::HashMap;
use std::mem;
use std::slice;
use std::time::Instant;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// Modo de ordenación y métrica para el Top de procesos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModoTopProcesos {
    Ram,
    Cpu,
}

/// Información resumida de un proceso en ejecución.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InfoProceso {
    pub pid: usize,
    pub nombre: String,
    pub memoria_bytes: usize,
    pub porcentaje_cpu: f64,
}

impl InfoProceso {
    /// Devuelve el texto formateado de consumo de memoria (ej. "450 MB" o "1.42 GB").
    pub fn memoria_formateada(&self) -> String {
        const MB: f64 = 1024.0 * 1024.0;
        const GB: f64 = 1024.0 * 1024.0 * 1024.0;
        let bytes = self.memoria_bytes as f64;
        if bytes >= GB {
            format!("{:.2} GB", bytes / GB)
        } else {
            format!("{:.0} MB", (bytes / MB).max(0.0))
        }
    }

    /// Devuelve el texto formateado de consumo de CPU (ej. "12.4%").
    pub fn cpu_formateada(&self) -> String {
        format!("{:>4.1}%", self.porcentaje_cpu.clamp(0.0, 100.0))
    }
}

/// Estado del recolector de procesos.
pub struct RecolectorProcesos {
    fn_nt_query: Option<FnNtQuerySystemInformation>,
    num_cpus: u32,
    ultimo_muestreo: Instant,
    historico_cpu: HashMap<usize, i64>, // PID -> tiempo total CPU (Kernel + User)
    buffer: Vec<u8>,
}

type FnNtQuerySystemInformation = unsafe extern "system" fn(
    u32,      // SystemInformationClass: 5 = SystemProcessInformation
    *mut u8,  // SystemInformation
    u32,      // SystemInformationLength
    *mut u32, // ReturnLength
) -> i32;

#[repr(C)]
#[allow(non_snake_case)]
struct UNICODE_STRING {
    Length: u16,
    MaximumLength: u16,
    Buffer: *const u16,
}

#[repr(C)]
#[allow(non_snake_case)]
struct SYSTEM_PROCESS_INFORMATION {
    NextEntryOffset: u32,
    NumberOfThreads: u32,
    WorkingSetPrivateSize: i64,
    HardFaultCount: u32,
    NumberOfThreadsHighWatermark: u32,
    CycleTime: u64,
    CreateTime: i64,
    UserTime: i64,
    KernelTime: i64,
    ImageName: UNICODE_STRING,
    BasePriority: i32,
    UniqueProcessId: usize,
    InheritedFromUniqueProcessId: usize,
    HandleCount: u32,
    SessionId: u32,
    UniqueProcessKey: usize,
    PeakVirtualSize: usize,
    VirtualSize: usize,
    PageFaultCount: u32,
    PeakWorkingSetSize: usize,
    WorkingSetSize: usize,
    QuotaPeakPagedPoolUsage: usize,
    QuotaPagedPoolUsage: usize,
    QuotaPeakNonPagedPoolUsage: usize,
    QuotaNonPagedPoolUsage: usize,
    PagefileUsage: usize,
    PeakPagefileUsage: usize,
    PrivatePageCount: usize,
    ReadOperationCount: i64,
    WriteOperationCount: i64,
    OtherOperationCount: i64,
    ReadTransferCount: i64,
    WriteTransferCount: i64,
    OtherTransferCount: i64,
}

impl RecolectorProcesos {
    pub fn nuevo() -> Self {
        let fn_nt_query = unsafe {
            let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
            if ntdll != 0 as _ {
                let proc = GetProcAddress(ntdll, b"NtQuerySystemInformation\0".as_ptr());
                if let Some(p) = proc {
                    Some(mem::transmute::<_, FnNtQuerySystemInformation>(p))
                } else {
                    None
                }
            } else {
                None
            }
        };

        let mut sys_info: SYSTEM_INFO = unsafe { mem::zeroed() };
        unsafe { GetSystemInfo(&mut sys_info) };
        let num_cpus = sys_info.dwNumberOfProcessors.max(1);

        Self {
            fn_nt_query,
            num_cpus,
            ultimo_muestreo: Instant::now(),
            historico_cpu: HashMap::with_capacity(256),
            buffer: vec![0u8; 256 * 1024], // 256 KB iniciales para la lista de procesos
        }
    }

    /// Actualiza y obtiene los 3 procesos con mayor consumo según el modo seleccionado.
    pub fn obtener_top_3(&mut self, modo: ModoTopProcesos) -> Vec<InfoProceso> {
        let fn_query = match self.fn_nt_query {
            Some(f) => f,
            None => return Vec::new(),
        };

        let ahora = Instant::now();
        let delta_tiempo = ahora.duration_since(self.ultimo_muestreo);
        let delta_100ns = (delta_tiempo.as_nanos() / 100) as f64;
        self.ultimo_muestreo = ahora;

        // Llamar a NtQuerySystemInformation redimensionando el buffer si es necesario
        let mut return_length = 0u32;
        loop {
            let status = unsafe {
                fn_query(
                    5, // SystemProcessInformation
                    self.buffer.as_mut_ptr(),
                    self.buffer.len() as u32,
                    &mut return_length,
                )
            };

            // 0xC0000004 = STATUS_INFO_LENGTH_MISMATCH
            if status == -1073741820 {
                let nuevo_tam = (return_length as usize + 64 * 1024).max(self.buffer.len() * 2);
                self.buffer.resize(nuevo_tam, 0);
            } else if status == 0 {
                break;
            } else {
                return Vec::new();
            }
        }

        let mut lista = Vec::with_capacity(128);
        let mut offset = 0usize;
        let mut nuevo_historico_cpu = HashMap::with_capacity(self.historico_cpu.len().max(128));

        loop {
            if offset + mem::size_of::<SYSTEM_PROCESS_INFORMATION>() > self.buffer.len() {
                break;
            }

            let entry = unsafe {
                &*(self.buffer.as_ptr().add(offset) as *const SYSTEM_PROCESS_INFORMATION)
            };

            let pid = entry.UniqueProcessId;

            // Omitir PID 0 (System Idle Process / Proceso inactivo del sistema)
            if pid != 0 {
                // Obtener nombre del proceso
                let nombre = if entry.ImageName.Length > 0 && !entry.ImageName.Buffer.is_null() {
                    let chars_count = (entry.ImageName.Length / 2) as usize;
                    let slice = unsafe {
                        slice::from_raw_parts(entry.ImageName.Buffer, chars_count)
                    };
                    String::from_utf16_lossy(slice)
                } else if pid == 4 {
                    "System".to_string()
                } else {
                    format!("PID {}", pid)
                };

                let tiempo_cpu_total = entry.KernelTime.saturating_add(entry.UserTime);
                nuevo_historico_cpu.insert(pid, tiempo_cpu_total);

                let pct_cpu = if delta_100ns > 0.0 {
                    if let Some(&ant_tiempo) = self.historico_cpu.get(&pid) {
                        let delta_cpu = tiempo_cpu_total.saturating_sub(ant_tiempo).max(0) as f64;
                        // Normalizado sobre el total del procesador (100% = todos los núcleos al máximo)
                        (delta_cpu / (delta_100ns * self.num_cpus as f64)) * 100.0
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                lista.push(InfoProceso {
                    pid,
                    nombre,
                    memoria_bytes: entry.WorkingSetSize,
                    porcentaje_cpu: pct_cpu,
                });
            }

            if entry.NextEntryOffset == 0 {
                break;
            }
            offset += entry.NextEntryOffset as usize;
        }

        self.historico_cpu = nuevo_historico_cpu;

        // Ordenar según el modo
        match modo {
            ModoTopProcesos::Ram => {
                lista.sort_by(|a, b| b.memoria_bytes.cmp(&a.memoria_bytes));
            }
            ModoTopProcesos::Cpu => {
                lista.sort_by(|a, b| {
                    b.porcentaje_cpu
                        .partial_cmp(&a.porcentaje_cpu)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        lista.truncate(3);
        lista
    }
}
