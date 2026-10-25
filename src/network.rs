//! Módulo de supervisión de red nativo para Windows.
//!
//! Utiliza la API IP Helper (`IpHlpApi.dll`) para obtener:
//! - Adaptador activo e interfaz de salida (Wi-Fi / Ethernet).
//! - Direcciones IPv4 e IPv6 locales.
//! - Velocidad de descarga y subida en tiempo real mediante deltas de bytes.
//! - Detección y diagnóstico de estado de conexiones VPN.
//!
//! No realiza ninguna petición externa respetando la privacidad.

use std::mem;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::slice;
use std::time::Instant;

use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GetIfEntry2, GAA_FLAG_INCLUDE_GATEWAYS,
    IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IF_TYPE_PROP_VIRTUAL,
    IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IP_ADAPTER_ADDRESSES_LH,
    MIB_IF_ROW2,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

/// Tipos de conexión física o virtual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoConexion {
    WiFi,
    Ethernet,
    VPN,
    Virtual,
    Otro,
    Desconectado,
}

impl TipoConexion {
    #[allow(dead_code)]
    pub fn etiqueta(&self) -> &'static str {
        match self {
            TipoConexion::WiFi => "Wi-Fi",
            TipoConexion::Ethernet => "Ethernet",
            TipoConexion::VPN => "VPN",
            TipoConexion::Virtual => "Virtual",
            TipoConexion::Otro => "Red",
            TipoConexion::Desconectado => "Sin Conexión",
        }
    }
}

/// Diagnóstico del estado de la VPN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoVPN {
    ConectadaEncaminada, // Hay un adaptador VPN activo y el tráfico sale por él
    DetectadaDirecta,    // Hay un adaptador VPN presente, pero el tráfico principal sale por Ethernet/Wi-Fi
    NoDetectada,         // No se han detectado adaptadores VPN
}

/// Información completa de red lista para presentar en la interfaz.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InfoRed {
    pub nombre_adaptador: String,
    pub tipo: TipoConexion,
    pub ipv4: String,
    pub ipv6: Option<String>,
    pub velocidad_descarga_bps: f64,
    pub velocidad_subida_bps: f64,
    pub estado_vpn: EstadoVPN,
    pub nombre_vpn: Option<String>,
    pub conectado: bool,
}

/// Formatea una tasa de transferencia en bytes por segundo a un texto legible (KB/s, MB/s).
pub fn formatear_velocidad(bytes_por_seg: f64) -> String {
    if bytes_por_seg >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes_por_seg / 1_048_576.0)
    } else if bytes_por_seg >= 1024.0 {
        format!("{:.0} KB/s", bytes_por_seg / 1024.0)
    } else {
        format!("{:.0} B/s", bytes_por_seg.max(0.0))
    }
}

/// Convierte una cadena UTF-16 terminada en nulo de Windows a `String` de Rust.
unsafe fn pwstr_a_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }
}

/// Recolector de red que mantiene los contadores de bytes anteriores
/// para calcular las velocidades de subida y descarga de forma fiable.
pub struct RecolectorRed {
    interfaz_activa_idx: Option<u32>,
    bytes_rx_prev: u64,
    bytes_tx_prev: u64,
    tiempo_prev: Option<Instant>,
    primera_lectura: bool,
}

impl RecolectorRed {
    pub fn nuevo() -> Self {
        Self {
            interfaz_activa_idx: None,
            bytes_rx_prev: 0,
            bytes_tx_prev: 0,
            tiempo_prev: None,
            primera_lectura: true,
        }
    }

    /// Actualiza y devuelve la información más reciente de la red.
    pub fn actualizar(&mut self) -> InfoRed {
        let (adaptadores, vpn_info) = self.obtener_adaptadores_windows();

        // 1. Seleccionamos el mejor adaptador activo
        let mejor_adaptador = adaptadores.into_iter().find(|a| a.es_principal);

        let ahora = Instant::now();

        if let Some(adaptador) = mejor_adaptador {
            let idx_actual = adaptador.if_index;

            // 2. Consultamos los contadores de bytes actuales para esta interfaz
            let (bytes_rx, bytes_tx) = self.obtener_octetos_interfaz(idx_actual);

            // 3. Calculamos la velocidad en bytes por segundo
            let (vel_rx, vel_tx) = if let (Some(t_prev), false) = (self.tiempo_prev, self.primera_lectura) {
                let delta_t = ahora.duration_since(t_prev).as_secs_f64();
                if delta_t > 0.05 && self.interfaz_activa_idx == Some(idx_actual) {
                    let rx = bytes_rx.saturating_sub(self.bytes_rx_prev) as f64 / delta_t;
                    let tx = bytes_tx.saturating_sub(self.bytes_tx_prev) as f64 / delta_t;
                    (rx, tx)
                } else {
                    (0.0, 0.0)
                }
            } else {
                (0.0, 0.0)
            };

            self.bytes_rx_prev = bytes_rx;
            self.bytes_tx_prev = bytes_tx;
            self.tiempo_prev = Some(ahora);
            self.interfaz_activa_idx = Some(idx_actual);
            self.primera_lectura = false;

            let (estado_vpn, nombre_vpn) = vpn_info;

            InfoRed {
                nombre_adaptador: adaptador.nombre,
                tipo: adaptador.tipo,
                ipv4: adaptador.ipv4.unwrap_or_else(|| "127.0.0.1".to_string()),
                ipv6: adaptador.ipv6,
                velocidad_descarga_bps: vel_rx,
                velocidad_subida_bps: vel_tx,
                estado_vpn,
                nombre_vpn,
                conectado: true,
            }
        } else {
            self.primera_lectura = true;
            InfoRed {
                nombre_adaptador: "Desconectado".to_string(),
                tipo: TipoConexion::Desconectado,
                ipv4: "Sin conexión".to_string(),
                ipv6: None,
                velocidad_descarga_bps: 0.0,
                velocidad_subida_bps: 0.0,
                estado_vpn: EstadoVPN::NoDetectada,
                nombre_vpn: None,
                conectado: false,
            }
        }
    }

    /// Lee los contadores de bytes recibidos y enviados para un índice de interfaz.
    fn obtener_octetos_interfaz(&self, if_index: u32) -> (u64, u64) {
        unsafe {
            let mut row: MIB_IF_ROW2 = mem::zeroed();
            row.InterfaceIndex = if_index;

            if GetIfEntry2(&mut row) == 0 {
                (row.InOctets, row.OutOctets)
            } else {
                (0, 0)
            }
        }
    }

    /// Invoca `GetAdaptersAddresses` de la API Win32 de Windows.
    fn obtener_adaptadores_windows(&self) -> (Vec<DatosAdaptadorInterno>, (EstadoVPN, Option<String>)) {
        let mut buffer: Vec<u8> = vec![0; 16384];
        let mut tamano: u32 = buffer.len() as u32;

        let flags = GAA_FLAG_INCLUDE_GATEWAYS;

        let resultado = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                flags,
                std::ptr::null(),
                buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut tamano,
            )
        };

        if resultado == ERROR_BUFFER_OVERFLOW {
            buffer.resize(tamano as usize, 0);
            let resultado_retry = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC as u32,
                    flags,
                    std::ptr::null(),
                    buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                    &mut tamano,
                )
            };
            if resultado_retry != 0 {
                return (Vec::new(), (EstadoVPN::NoDetectada, None));
            }
        } else if resultado != 0 {
            return (Vec::new(), (EstadoVPN::NoDetectada, None));
        }

        let mut adaptadores = Vec::new();
        let mut vpn_detectada: Option<String> = None;
        let mut vpn_encaminada = false;

        unsafe {
            let mut p_actual = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;

            while !p_actual.is_null() {
                let actual = &*p_actual;

                let nombre_amigable = pwstr_a_string(actual.FriendlyName);
                let descripcion = pwstr_a_string(actual.Description);
                let if_type = actual.IfType;
                let oper_status = actual.OperStatus; // 1 = IfOperStatusUp
                let if_index = actual.Anonymous1.Anonymous.IfIndex;

                let es_operativo = oper_status == 1; // 1 = Up
                let tiene_gateway = !actual.FirstGatewayAddress.is_null();

                // Detección de firmas de VPN conocidas
                let nombre_lower = nombre_amigable.to_lowercase();
                let desc_lower = descripcion.to_lowercase();
                let es_firma_vpn = if_type == IF_TYPE_TUNNEL
                    || if_type == IF_TYPE_PROP_VIRTUAL
                    || nombre_lower.contains("vpn")
                    || nombre_lower.contains("wireguard")
                    || nombre_lower.contains("wintun")
                    || nombre_lower.contains("tailscale")
                    || nombre_lower.contains("zerotier")
                    || desc_lower.contains("tap-windows")
                    || desc_lower.contains("openvpn")
                    || desc_lower.contains("wireguard");

                if es_firma_vpn && es_operativo {
                    vpn_detectada = Some(nombre_amigable.clone());
                    if tiene_gateway {
                        vpn_encaminada = true;
                    }
                }

                // Descartamos loopbacks
                if if_type != IF_TYPE_SOFTWARE_LOOPBACK && es_operativo {
                    let mut ipv4_encontrada: Option<String> = None;
                    let mut ipv6_encontrada: Option<String> = None;

                    // Iterar direcciones unicast del adaptador
                    let mut p_unicast = actual.FirstUnicastAddress;
                    while !p_unicast.is_null() {
                        let unicast = &*p_unicast;
                        let sockaddr = unicast.Address.lpSockaddr;

                        if !sockaddr.is_null() {
                            let familia = (*sockaddr).sa_family;
                            if familia == AF_INET {
                                let sa_in = sockaddr as *const SOCKADDR_IN;
                                let octetos = (*sa_in).sin_addr.S_un.S_un_b;
                                let ip = Ipv4Addr::new(octetos.s_b1, octetos.s_b2, octetos.s_b3, octetos.s_b4);
                                // Evitar 127.0.0.1 o IPs de autoconfiguración 169.254.x.x
                                if !ip.is_loopback() && !ip.is_link_local() {
                                    ipv4_encontrada = Some(ip.to_string());
                                }
                            } else if familia == AF_INET6 && ipv6_encontrada.is_none() {
                                let sa_in6 = sockaddr as *const SOCKADDR_IN6;
                                let ip6 = Ipv6Addr::from((*sa_in6).sin6_addr.u.Byte);
                                if !ip6.is_loopback() && !ip6.is_unicast_link_local() {
                                    ipv6_encontrada = Some(ip6.to_string());
                                }
                            }
                        }
                        p_unicast = unicast.Next;
                    }

                    let tipo = match if_type {
                        IF_TYPE_IEEE80211 => TipoConexion::WiFi,
                        IF_TYPE_ETHERNET_CSMACD => {
                            if es_firma_vpn {
                                TipoConexion::VPN
                            } else if desc_lower.contains("virtual") || desc_lower.contains("hyper-v") {
                                TipoConexion::Virtual
                            } else {
                                TipoConexion::Ethernet
                            }
                        }
                        IF_TYPE_TUNNEL | IF_TYPE_PROP_VIRTUAL => TipoConexion::VPN,
                        _ => TipoConexion::Otro,
                    };

                    if let Some(ref ip4) = ipv4_encontrada {
                        // Un adaptador se considera principal si está operativo, tiene gateway y no es puramente virtual
                        let es_principal = tiene_gateway && tipo != TipoConexion::Virtual;

                        adaptadores.push(DatosAdaptadorInterno {
                            if_index,
                            nombre: nombre_amigable,
                            tipo,
                            ipv4: Some(ip4.clone()),
                            ipv6: ipv6_encontrada,
                            tiene_gateway,
                            es_principal,
                        });
                    }
                }

                p_actual = actual.Next;
            }
        }

        // Si ningún adaptador fue marcado con gateway (caso de redes directas), elegimos el primer Wi-Fi o Ethernet
        if !adaptadores.iter().any(|a| a.es_principal) {
            if let Some(primer_candidato) = adaptadores.iter_mut().find(|a| a.tipo == TipoConexion::WiFi || a.tipo == TipoConexion::Ethernet) {
                primer_candidato.es_principal = true;
            } else if let Some(primero) = adaptadores.first_mut() {
                primero.es_principal = true;
            }
        }

        let estado_vpn = match vpn_detectada.as_ref() {
            Some(_) if vpn_encaminada => EstadoVPN::ConectadaEncaminada,
            Some(_) => EstadoVPN::DetectadaDirecta,
            None => EstadoVPN::NoDetectada,
        };

        (adaptadores, (estado_vpn, vpn_detectada))
    }
}

/// Estructura interna para filtrar y ordenar adaptadores candidatos.
struct DatosAdaptadorInterno {
    if_index: u32,
    nombre: String,
    tipo: TipoConexion,
    ipv4: Option<String>,
    ipv6: Option<String>,
    #[allow(dead_code)]
    tiene_gateway: bool,
    es_principal: bool,
}
