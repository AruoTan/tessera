//! Local WMI sensors and Windows thermal zones on an isolated thread. A slow provider cannot
//! delay network/CPU/memory sampling or the taskbar's message loop.
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tessera_core::hardware::thermal::{ThermalUnit, ThermalZones};
use tessera_core::hardware::{TemperatureSensor, cpu_temperature};
use windows::{
    Win32::System::{
        Com::*,
        Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
        Variant::VARIANT,
        Wmi::*,
    },
    core::{BSTR, HRESULT, PCWSTR, Result, w},
};

mod pdh;

struct Reading {
    at: Instant,
    celsius: Option<f64>,
}

pub struct Reader(Arc<RwLock<Reading>>);

impl Reader {
    pub fn start(shutdown: Arc<AtomicBool>) -> std::io::Result<Self> {
        let latest = Arc::new(RwLock::new(Reading {
            at: Instant::now(),
            celsius: None,
        }));
        let output = latest.clone();
        thread::Builder::new()
            .name("cpu-temperature".into())
            .spawn(move || {
                if let Err(error) = run(shutdown, output) {
                    eprintln!("CPU temperature provider unavailable: {error}");
                }
            })?;
        Ok(Self(latest))
    }

    pub fn latest(&self) -> Option<f64> {
        let reading = self.0.read().ok()?;
        (reading.at.elapsed() <= Duration::from_secs(5))
            .then_some(reading.celsius)
            .flatten()
    }
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balanced with successful initialization on this same thread,
        // after all WMI objects have been released.
        unsafe { CoUninitialize() };
    }
}

struct Provider {
    namespace: &'static str,
    kind: QueryKind,
    service: Option<IWbemServices>,
    retry_at: Instant,
}

#[derive(Clone, Copy)]
enum QueryKind {
    Sensors,
    Acpi,
}

impl Provider {
    fn new(namespace: &'static str, kind: QueryKind) -> Self {
        Self {
            namespace,
            kind,
            service: None,
            retry_at: Instant::now(),
        }
    }

    fn read(&mut self, locator: &IWbemLocator, zones: &mut ThermalZones) -> Option<f64> {
        if self.service.is_none() && Instant::now() < self.retry_at {
            return None;
        }
        let result = (|| {
            if self.service.is_none() {
                self.service = Some(connect(locator, self.namespace)?);
            }
            let service = self.service.as_ref().expect("provider connected");
            match self.kind {
                QueryKind::Sensors => query_sensors(service),
                QueryKind::Acpi => query_acpi(service, zones),
            }
        })();
        match result {
            Ok(value) => value,
            Err(_) => {
                self.service = None;
                self.retry_at = Instant::now() + Duration::from_secs(10);
                None
            }
        }
    }
}

fn run(shutdown: Arc<AtomicBool>, latest: Arc<RwLock<Reading>>) -> Result<()> {
    // SAFETY: this dedicated thread owns its COM apartment and every proxy.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let _apartment = ComApartment;
    let mut locator: Option<IWbemLocator> = None;
    let mut retry_locator_at = Instant::now();
    let mut providers = ["ROOT\\LibreHardwareMonitor", "ROOT\\OpenHardwareMonitor"]
        .map(|namespace| Provider::new(namespace, QueryKind::Sensors));
    let mut pdh = pdh::Reader::default();
    let mut acpi = Provider::new("ROOT\\WMI", QueryKind::Acpi);
    let mut zones = ThermalZones::default();
    let mut last_source = None;
    while !shutdown.load(Ordering::Relaxed) {
        let next = Instant::now() + Duration::from_secs(1);
        if locator.is_none() && Instant::now() >= retry_locator_at {
            // WMI may be disabled or still starting. PDH remains usable without
            // a WMI locator; retry instead of terminating the temperature worker.
            locator = unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }.ok();
            retry_locator_at = Instant::now() + Duration::from_secs(10);
        }
        let selected = locator
            .as_ref()
            .and_then(|locator| {
                providers.iter_mut().find_map(|provider| {
                    provider
                        .read(locator, &mut zones)
                        .map(|value| (provider.namespace, value))
                })
            })
            .or_else(|| pdh.read().map(|value| ("PDH ACPI thermal zone", value)))
            .or_else(|| {
                locator.as_ref().and_then(|locator| {
                    acpi.read(locator, &mut zones)
                        .map(|value| ("WMI ACPI thermal zone", value))
                })
            });
        let source = selected.map(|(source, _)| source);
        if source != last_source {
            if let Some((source, value)) = selected {
                eprintln!("CPU temperature: {value:.1} °C via {source}");
            }
            last_source = source;
        }
        let celsius = selected.map(|(_, value)| value);
        if let Ok(mut reading) = latest.write() {
            *reading = Reading {
                at: Instant::now(),
                celsius,
            };
        }
        while !shutdown.load(Ordering::Relaxed) {
            let remaining = next.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::park_timeout(remaining);
        }
    }
    Ok(())
}

fn connect(locator: &IWbemLocator, namespace: &str) -> Result<IWbemServices> {
    // SAFETY: local-only namespace, default credentials, no borrowed buffers
    // retained. Bound connection wait and authenticate the owned service proxy.
    unsafe {
        let empty = BSTR::new();
        let service = locator.ConnectServer(
            &BSTR::from(namespace),
            &empty,
            &empty,
            &empty,
            WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
            &empty,
            None,
        )?;
        CoSetProxyBlanket(
            &service,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        Ok(service)
    }
}

fn property(object: &IWbemClassObject, name: PCWSTR) -> Result<VARIANT> {
    let mut value = VARIANT::default();
    // SAFETY: VARIANT owns its BSTR/value and is cleared automatically on drop.
    unsafe { object.Get(name, 0, &mut value, None, None)? };
    Ok(value)
}

fn query_sensors(service: &IWbemServices) -> Result<Option<f64>> {
    let objects = query(
        service,
        "SELECT Name, Identifier, Value FROM Sensor WHERE SensorType='Temperature'",
    )?;
    Ok(cpu_temperature(objects.into_iter().filter_map(|object| {
        Some(TemperatureSensor {
            identifier: BSTR::try_from(&property(&object, w!("Identifier")).ok()?)
                .ok()?
                .to_string(),
            name: BSTR::try_from(&property(&object, w!("Name")).ok()?)
                .ok()?
                .to_string(),
            celsius: property(&object, w!("Value"))
                .ok()
                .and_then(|value| f64::try_from(&value).ok()),
        })
    })))
}

fn query_acpi(service: &IWbemServices, zones: &mut ThermalZones) -> Result<Option<f64>> {
    let objects = query(
        service,
        "SELECT InstanceName, CurrentTemperature FROM MSAcpi_ThermalZoneTemperature",
    )?;
    Ok(objects
        .into_iter()
        .filter_map(|object| {
            let name = BSTR::try_from(&property(&object, w!("InstanceName")).ok()?)
                .ok()?
                .to_string();
            let raw = f64::try_from(&property(&object, w!("CurrentTemperature")).ok()?).ok()?;
            zones.read(&name, raw, ThermalUnit::Decikelvin)
        })
        .max_by(f64::total_cmp))
}

fn query(service: &IWbemServices, text: &str) -> Result<Vec<IWbemClassObject>> {
    // SAFETY: synchronous use of apartment-local COM proxies and owned output
    // slots. Enumeration is bounded; a stalled result is discarded in full.
    unsafe {
        let results = service.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(text),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut collected = Vec::new();
        loop {
            if Instant::now() >= deadline || collected.len() >= 1024 {
                return Err(windows::core::Error::from_hresult(HRESULT(
                    0x800705b4_u32 as i32,
                )));
            }
            let mut objects = [None];
            let mut returned = 0;
            let status = results.Next(100, &mut objects, &mut returned);
            status.ok()?;
            // WBEM_S_FALSE marks a complete enumeration; TIMEDOUT is also a
            // success HRESULT, but it must never publish a partial sensor set.
            if returned == 0 && status.0 == WBEM_S_FALSE.0 {
                break;
            }
            if returned != 1 {
                return Err(windows::core::Error::from_hresult(HRESULT(
                    0x800705b4_u32 as i32,
                )));
            }
            if let Some(object) = objects[0].take() {
                collected.push(object);
            }
        }
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_or_disconnected_provider_cannot_leave_a_stale_temperature() {
        let reader = Reader(Arc::new(RwLock::new(Reading {
            at: Instant::now(),
            celsius: Some(52.0),
        })));
        assert_eq!(reader.latest(), Some(52.0));
        reader.0.write().unwrap().at = Instant::now() - Duration::from_secs(6);
        assert_eq!(reader.latest(), None);
        *reader.0.write().unwrap() = Reading {
            at: Instant::now(),
            celsius: None,
        };
        assert_eq!(reader.latest(), None);
        reader.0.write().unwrap().celsius = Some(55.0);
        assert_eq!(reader.latest(), Some(55.0));
    }
}
