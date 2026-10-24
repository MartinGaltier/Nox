//! The shared sampler: opens every handle once, reuses it on each tick and
//! releases everything on drop (end of the worker thread).

use crate::battery::{self, BatteryInputs};
use crate::cpu;
use crate::disk;
use crate::memory;
use crate::network::NetTracker;
use crate::power::{self, BlockPower, EnergyTracker};
use crate::sensors::{self, Platform, Role, Sample};
use crate::smc::{decode, fourcc, fourcc_str};
use crate::sys::disk as sysdisk;
use crate::sys::iokit::{self, IoObject};
use crate::sys::ioreport::Subscription;
use crate::sys::mach::{self, CpuTicks};
use crate::sys::net::{self, DynamicStore};
use crate::sys::smc::{KeyInfo, Smc};
use crate::value::Value;
use nox_core::model::metrics::{CoreKind, MetricsSnapshot, PowerInfo};
use nox_core::{Reading, Unavailable};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

struct SmcKey {
    code: u32,
    info: KeyInfo,
}

struct TempKey {
    key: SmcKey,
    name: String,
    role: Role,
}

struct SmcState {
    smc: Smc,
    temps: Vec<TempKey>,
    system_power: Option<SmcKey>,
    adapter_power: Option<SmcKey>,
    fans: Result<Vec<SmcKey>, Unavailable>,
}

impl SmcState {
    fn open(platform: Platform) -> Result<Self, String> {
        let smc = Smc::open()?;
        let lookup = |name: &str| {
            let code = fourcc(name);
            smc.key_info(code).map(|info| SmcKey { code, info })
        };
        // Enumerate once; afterwards only the selected keys are read.
        let mut temps = vec![];
        for i in 0..smc.key_count() {
            let Some(code) = smc.key_at(i) else { continue };
            let name = fourcc_str(code);
            let Some(role) = sensors::classify(&name, platform) else {
                continue;
            };
            let Some(info) = smc.key_info(code) else {
                continue;
            };
            if &info.kind != b"flt " {
                continue;
            }
            temps.push(TempKey {
                key: SmcKey { code, info },
                name,
                role,
            });
        }
        let system_power = lookup("PSTR");
        let adapter_power = lookup("PDTR");
        let fans = match lookup("FNum") {
            None => Err(Unavailable::NotSupported),
            Some(k) => {
                let n = smc
                    .read(k.code, k.info)
                    .and_then(|b| decode(&b, &k.info.kind))
                    .unwrap_or(0.0) as u32;
                if n == 0 {
                    Err(Unavailable::NotSupported)
                } else {
                    (0..n.min(8))
                        .map(|i| lookup(&format!("F{i}Ac")))
                        .collect::<Option<Vec<_>>>()
                        .ok_or(Unavailable::Error("fan keys missing".into()))
                }
            }
        };
        Ok(Self {
            smc,
            temps,
            system_power,
            adapter_power,
            fans,
        })
    }

    fn value(&self, k: &SmcKey) -> Option<f64> {
        self.smc
            .read(k.code, k.info)
            .and_then(|b| decode(&b, &k.info.kind))
    }
}

pub struct Collector {
    clock: Instant,
    platform: Platform,
    prev_ticks: Option<(Vec<CpuTicks>, f64)>,
    core_kinds: Vec<CoreKind>,
    gpu: Option<IoObject>,
    smc: Result<SmcState, String>,
    total_mem: Option<u64>,
    page_size: u64,
    battery: Option<IoObject>,
    ioreport: Result<Subscription, String>,
    energy: EnergyTracker,
    last_energy_at: Option<f64>,
    store: Option<DynamicStore>,
    net: NetTracker,
    volume_names: HashMap<String, Option<String>>,
}

fn registry_core_kinds() -> Vec<(u32, CoreKind)> {
    IoObject::all_matching("IOPlatformDevice")
        .iter()
        .filter_map(|dev| {
            let kind = match dev.property("cluster-type")?.to_value() {
                Value::Data(d) => match d.first() {
                    Some(b'E') => CoreKind::Efficiency,
                    Some(b'P') => CoreKind::Performance,
                    _ => return None,
                },
                _ => return None,
            };
            let id = dev.property("logical-cpu-id")?.to_value().as_i64()?;
            Some((u32::try_from(id).ok()?, kind))
        })
        .collect()
}

fn find_gpu() -> Option<IoObject> {
    IoObject::all_matching("IOAccelerator")
        .into_iter()
        .find(|a| gpu_usage(a).is_some())
}

/// "Device Utilization %" of the accelerator's PerformanceStatistics (only
/// that property is copied, not the whole registry entry).
fn gpu_usage(acc: &IoObject) -> Option<f64> {
    let v = acc.property("PerformanceStatistics")?.to_value();
    let u = v.int("Device Utilization %")?;
    (0..=100).contains(&u).then_some(u as f64)
}

impl Collector {
    pub fn new() -> Self {
        let platform = Platform::from_brand(
            &mach::sysctl_string("machdep.cpu.brand_string").unwrap_or_default(),
        );
        let n_cpus = mach::cpu_ticks().map(|t| t.len()).unwrap_or(0);
        let e = mach::sysctl_i32("hw.perflevel1.logicalcpu").map(|v| v.max(0) as usize);
        let core_kinds = cpu::core_kinds(&registry_core_kinds(), n_cpus, e);
        Self {
            clock: Instant::now(),
            platform,
            prev_ticks: None,
            core_kinds,
            gpu: find_gpu(),
            smc: SmcState::open(platform),
            total_mem: mach::sysctl_u64("hw.memsize"),
            page_size: mach::sysctl_u64("hw.pagesize")
                .or_else(|| mach::sysctl_i32("hw.pagesize").map(|v| v as u64))
                .unwrap_or(16_384),
            battery: IoObject::matching("AppleSmartBattery"),
            ioreport: Subscription::new("Energy Model"),
            energy: EnergyTracker::default(),
            last_energy_at: None,
            store: DynamicStore::new(),
            net: NetTracker::default(),
            volume_names: HashMap::new(),
        }
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// One full snapshot. `max_gap`: seconds beyond which deltas (CPU,
    /// network, energy) are stale (sleep/pause) and only re-baseline.
    pub fn sample(&mut self, max_gap: f64) -> MetricsSnapshot {
        let now = self.clock.elapsed().as_secs_f64();
        let (cpu_total, cpu_cores) = self.cpu(now, max_gap);
        let gpu_usage = self.gpu();
        let temps = self.temperatures();
        let (cpu_temp, gpu_temp, sensors_r, battery_temp) = match &temps {
            Ok(s) => (
                opt(sensors::cpu_temp(s)),
                opt(sensors::gpu_temp(s)),
                Reading::Measured(sensors::shown(s)),
                sensors::battery_temp(s),
            ),
            Err(e) => (
                Reading::Unavailable(Unavailable::Error(e.clone())),
                Reading::Unavailable(Unavailable::Error(e.clone())),
                Reading::Unavailable(Unavailable::Error(e.clone())),
                None,
            ),
        };
        let battery = self.battery(battery_temp);
        let power = self.power(now, max_gap, &battery);
        let fans_rpm = match &self.smc {
            Err(e) => Reading::Unavailable(Unavailable::Error(e.clone())),
            Ok(s) => match &s.fans {
                Err(u) => Reading::Unavailable(u.clone()),
                Ok(keys) => keys
                    .iter()
                    .map(|k| s.value(k))
                    .collect::<Option<Vec<_>>>()
                    .map_or(
                        Reading::Unavailable(Unavailable::Error("fan read".into())),
                        Reading::Measured,
                    ),
            },
        };
        MetricsSnapshot {
            taken_at: nox_core::model::now(),
            cpu_total,
            cpu_cores,
            gpu_usage,
            cpu_temp,
            gpu_temp,
            sensors: sensors_r,
            memory: self.memory(),
            battery,
            power,
            network: self.network(now, max_gap),
            disks: self.disks(),
            fans_rpm,
        }
    }

    fn cpu(
        &mut self,
        now: f64,
        max_gap: f64,
    ) -> (
        Reading<f64>,
        Reading<Vec<nox_core::model::metrics::CoreUsage>>,
    ) {
        let ticks = match mach::cpu_ticks() {
            Ok(t) => t,
            Err(e) => {
                self.prev_ticks = None;
                let u = Unavailable::Error(e);
                return (Reading::Unavailable(u.clone()), Reading::Unavailable(u));
            }
        };
        if self.core_kinds.len() != ticks.len() {
            let e = mach::sysctl_i32("hw.perflevel1.logicalcpu").map(|v| v.max(0) as usize);
            self.core_kinds = cpu::core_kinds(&registry_core_kinds(), ticks.len(), e);
        }
        let prev = self.prev_ticks.replace((ticks.clone(), now));
        match prev {
            Some((p, t)) if now - t <= max_gap => match cpu::usage(&p, &ticks) {
                Some((total, per)) => (
                    Reading::Measured(total),
                    Reading::Measured(cpu::cores(&per, &self.core_kinds)),
                ),
                None => (Reading::pending(), Reading::pending()),
            },
            _ => (Reading::pending(), Reading::pending()),
        }
    }

    fn gpu(&mut self) -> Reading<f64> {
        if let Some(v) = self.gpu.as_ref().and_then(gpu_usage) {
            return Reading::Measured(v);
        }
        // Accelerator gone or not yet published: look again once.
        self.gpu = find_gpu();
        match self.gpu.as_ref().and_then(gpu_usage) {
            Some(v) => Reading::Measured(v),
            None => Reading::Unavailable(Unavailable::NotSupported),
        }
    }

    fn temperatures(&self) -> Result<Vec<Sample>, String> {
        let s = self.smc.as_ref().map_err(Clone::clone)?;
        Ok(s.temps
            .iter()
            .filter_map(|t| {
                let c = s.value(&t.key)?;
                sensors::plausible(t.role, c).then(|| Sample {
                    key: t.name.clone(),
                    role: t.role,
                    celsius: c,
                })
            })
            .collect())
    }

    fn memory(&self) -> Reading<nox_core::model::metrics::MemoryInfo> {
        let Some(total) = self.total_mem else {
            return Reading::Unavailable(Unavailable::Error("hw.memsize".into()));
        };
        match mach::vm_stats() {
            Ok(p) => Reading::Measured(memory::info(
                total,
                self.page_size,
                p,
                memory::pressure(mach::sysctl_i32("kern.memorystatus_vm_pressure_level")),
                mach::swap_usage().unwrap_or((0, 0)),
            )),
            Err(e) => Reading::Unavailable(Unavailable::Error(e)),
        }
    }

    fn battery(&self, smc_temp: Option<f64>) -> Reading<nox_core::model::metrics::BatteryInfo> {
        let registry = self.battery.as_ref().map(|b| {
            let mut m = BTreeMap::new();
            for k in battery::REGISTRY_KEYS {
                if let Some(v) = b.property(k) {
                    m.insert(k.to_owned(), v.to_value());
                }
            }
            Value::Dict(m)
        });
        let ps = iokit::power_sources();
        let source = ps
            .as_ref()
            .and_then(|p| battery::internal_source(&p.descriptions));
        let adapter = match &self.smc {
            Ok(s) => s.adapter_power.as_ref().and_then(|k| s.value(k)),
            Err(_) => None,
        };
        battery::decode(&BatteryInputs {
            registry: registry.as_ref(),
            source,
            providing: ps.as_ref().and_then(|p| p.providing.as_deref()),
            smc_temp_c: smc_temp,
            smc_adapter_w: adapter,
        })
    }

    fn power(
        &mut self,
        now: f64,
        max_gap: f64,
        battery: &Reading<nox_core::model::metrics::BatteryInfo>,
    ) -> Reading<PowerInfo> {
        let pstr = match &self.smc {
            Ok(s) => s.system_power.as_ref().and_then(|k| s.value(k)),
            Err(_) => None,
        };
        let b = battery.value();
        let system_watts = power::system_watts(
            pstr,
            b.and_then(|b| b.battery_watts.value().copied()),
            b.is_some_and(|b| b.on_ac),
        );
        let blocks = match &mut self.ioreport {
            Err(e) => BlockPower::unavailable(Unavailable::Error(e.clone())),
            Ok(sub) => match sub.sample() {
                Err(e) => BlockPower::unavailable(Unavailable::Error(e)),
                Ok(deltas) => {
                    let prev = self.last_energy_at.replace(now);
                    match prev {
                        Some(t) if now - t <= max_gap => self.energy.interpret(&deltas, now - t),
                        _ => BlockPower::unavailable(Unavailable::Pending),
                    }
                }
            },
        };
        Reading::Measured(PowerInfo {
            system_watts,
            cpu_watts: blocks.cpu,
            gpu_watts: blocks.gpu,
            ane_watts: blocks.ane,
        })
    }

    fn network(
        &mut self,
        now: f64,
        max_gap: f64,
    ) -> Reading<nox_core::model::metrics::NetworkInfo> {
        let primary = self
            .store
            .as_ref()
            .and_then(DynamicStore::primary_interface);
        match net::interface_counters() {
            Ok(c) => self.net.sample(primary.as_deref(), &c, now, max_gap),
            Err(e) => Reading::Unavailable(Unavailable::Error(e)),
        }
    }

    fn disks(&mut self) -> Reading<Vec<nox_core::model::metrics::DiskInfo>> {
        match sysdisk::mounts() {
            Err(e) => Reading::Unavailable(Unavailable::Error(e)),
            Ok(mounts) => {
                self.volume_names
                    .retain(|k, _| mounts.iter().any(|m| &m.mount == k));
                Reading::Measured(
                    mounts
                        .iter()
                        .filter(|m| disk::shown(m))
                        .map(|m| {
                            let name = self
                                .volume_names
                                .entry(m.mount.clone())
                                .or_insert_with(|| sysdisk::volume_name(&m.mount))
                                .clone();
                            disk::info(m, name)
                        })
                        .collect(),
                )
            }
        }
    }
}

fn opt(v: Option<f64>) -> Reading<f64> {
    v.map_or(
        Reading::Unavailable(Unavailable::NotSupported),
        Reading::Measured,
    )
}
