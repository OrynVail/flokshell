use std::ffi::{CString, OsStr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

use crate::bus::{Bus, Request};

const SERVICE: &str = "resources";

// The shell renews the watch with every sample, so a shell that dies mid-watch
// stops the sampling without saying so.
const LEASE_INTERVALS: u32 = 3;
const LEASE_MIN: Duration = Duration::from_secs(5);
const MIN_INTERVAL: Duration = Duration::from_millis(100);

const CPU_HWMON: [&str; 6] = [
    "coretemp",
    "k10temp",
    "zenpower",
    "cpu_thermal",
    "x86_pkg_temp",
    "amd_energy",
];

// Not on the default search path under NixOS.
const NVML: [&str; 2] = [
    "libnvidia-ml.so.1",
    "/run/opengl-driver/lib/libnvidia-ml.so.1",
];

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    cpu_model: String,
    cpu_usage: f64,
    cpu_temp_c: i32,
    ram_total: u64,
    ram_used: u64,
    ram_available: u64,
    gpus: Vec<Gpu>,
    disks: Vec<Disk>,
    seq: u64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Gpu {
    name: String,
    vendor: String,
    usage: f64,
    temp_c: i32,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Disk {
    mount: String,
    kind: String,
    usage: f64,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Watch {
        interval_ms: u64,
        disks: Vec<String>,
    },
    Stop,
}

impl TryFrom<Request> for Command {
    type Error = serde_json::Error;

    fn try_from(req: Request) -> Result<Self, Self::Error> {
        let mut v = serde_json::json!({ "cmd": req.cmd });
        if !req.args.is_null() {
            v["args"] = req.args;
        }
        serde_json::from_value(v)
    }
}

#[derive(Clone, Debug)]
enum Source {
    Nvidia { pci: String, power: PathBuf },
    Amd { device: PathBuf },
    Intel,
}

#[derive(Clone, Debug)]
struct Card {
    name: String,
    source: Source,
}

impl Card {
    fn vendor(&self) -> &'static str {
        match self.source {
            Source::Nvidia { .. } => "nvidia",
            Source::Amd { .. } => "amd",
            Source::Intel => "intel",
        }
    }
}

struct Sampler {
    cards: Vec<Card>,
    disks: Vec<(String, String)>,
    prev_cpu: Option<CpuTimes>,
    // Held for a watch and dropped with it: an open handle keeps the dGPU awake.
    nvml: Option<Nvml>,
}

impl Sampler {
    fn sample(&mut self, state: &mut State) {
        let cpu = std::fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|s| cpu_times(&s));
        state.cpu_usage = match (self.prev_cpu, cpu) {
            (Some(prev), Some(cur)) => cpu_usage(prev, cur),
            _ => 0.0,
        };
        self.prev_cpu = cpu.or(self.prev_cpu);
        state.cpu_temp_c = cpu_temp();

        if let Some(m) = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| meminfo(&s))
        {
            state.ram_total = m.total;
            state.ram_available = m.available;
            state.ram_used = m.total.saturating_sub(m.available);
        }

        state.gpus = self
            .cards
            .clone()
            .iter()
            .map(|card| {
                let (usage, temp_c) = self.gpu(card);
                Gpu {
                    name: card.name.clone(),
                    vendor: card.vendor().into(),
                    usage,
                    temp_c,
                }
            })
            .collect();

        state.disks = self
            .disks
            .iter()
            .map(|(mount, kind)| Disk {
                mount: mount.clone(),
                kind: kind.clone(),
                usage: disk_usage(mount).unwrap_or(0.0),
            })
            .collect();
    }

    fn gpu(&mut self, card: &Card) -> (f64, i32) {
        match &card.source {
            Source::Nvidia { pci, power } => {
                let awake = std::fs::read_to_string(power)
                    .map(|s| s.trim() == "active")
                    .unwrap_or(true);
                if !awake {
                    return (0.0, -1);
                }
                let Some(nvml) = self.nvml() else {
                    return (0.0, -1);
                };
                let Ok(dev) = nvml.device_by_pci_bus_id(pci.as_str()) else {
                    return (0.0, -1);
                };
                (
                    dev.utilization_rates().map_or(0.0, |u| u.gpu as f64),
                    dev.temperature(TemperatureSensor::Gpu)
                        .map_or(-1, |t| t as i32),
                )
            }
            Source::Amd { device } => (
                read_trimmed(&device.join("gpu_busy_percent"))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0),
                first_hwmon(&device.join("hwmon"))
                    .and_then(|h| read_trimmed(&h.join("temp1_input")))
                    .and_then(|s| s.parse::<i32>().ok())
                    .map_or(-1, |m| m / 1000),
            ),
            Source::Intel => (0.0, -1),
        }
    }

    fn nvml(&mut self) -> Option<&Nvml> {
        if self.nvml.is_none() {
            self.nvml = NVML.iter().find_map(|lib| {
                Nvml::builder()
                    .lib_path(OsStr::new(lib))
                    .init()
                    .inspect_err(|e| tracing::debug!(lib, "nvml: {e}"))
                    .ok()
            });
            if self.nvml.is_none() {
                tracing::warn!("nvml unavailable, nvidia stats will read zero");
            }
        }
        self.nvml.as_ref()
    }
}

pub struct Service {
    bus: Bus,
    state: State,
    cards: Vec<Card>,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        let state = State {
            cpu_model: std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|s| cpu_model(&s))
                .unwrap_or_else(|| "Unknown CPU".into()),
            cpu_temp_c: -1,
            ..Default::default()
        };
        Self {
            bus,
            state,
            cards: detect_gpus(),
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        self.state.gpus = self
            .cards
            .iter()
            .map(|c| Gpu {
                name: c.name.clone(),
                vendor: c.vendor().into(),
                usage: 0.0,
                temp_c: -1,
            })
            .collect();
        self.publish();

        let mut sampler: Option<Sampler> = None;
        let mut lease = Instant::now();
        // procfs and sysfs have no change signal; polled only while a watch is held.
        let mut every = MIN_INTERVAL;
        let mut tick = tokio::time::interval(every);

        loop {
            tokio::select! {
                Some(req) = requests.recv() => match Command::try_from(req) {
                    Ok(Command::Watch { interval_ms, disks }) => {
                        let next = Duration::from_millis(interval_ms).max(MIN_INTERVAL);
                        lease = Instant::now() + (next * LEASE_INTERVALS).max(LEASE_MIN);

                        let restart = sampler.is_none() || next != every;
                        match &mut sampler {
                            Some(s) => {
                                if !s.disks.iter().map(|(m, _)| m).eq(disks.iter()) {
                                    s.disks = with_kinds(disks);
                                }
                            }
                            None => {
                                let mut s = Sampler {
                                    cards: self.cards.clone(),
                                    disks: with_kinds(disks),
                                    prev_cpu: None,
                                    nvml: None,
                                };
                                s.prev_cpu = std::fs::read_to_string("/proc/stat")
                                    .ok()
                                    .and_then(|s| cpu_times(&s));
                                sampler = Some(s);
                                tracing::debug!("watch started");
                            }
                        }
                        if restart {
                            every = next;
                            tick = tokio::time::interval_at(Instant::now() + every, every);
                            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
                        }
                    }
                    Ok(Command::Stop) => {
                        if sampler.take().is_some() {
                            tracing::debug!("watch stopped");
                        }
                    }
                    Err(e) => tracing::warn!("bad command: {e}"),
                },
                _ = tick.tick(), if sampler.is_some() => {
                    if Instant::now() > lease {
                        tracing::debug!("watch lapsed");
                        sampler = None;
                        continue;
                    }
                    let mut s = sampler.take().unwrap();
                    let mut state = self.state.clone();
                    match tokio::task::spawn_blocking(move || {
                        s.sample(&mut state);
                        (s, state)
                    })
                    .await
                    {
                        Ok((s, state)) => {
                            self.state = State {
                                seq: self.state.seq + 1,
                                ..state
                            };
                            self.publish();
                            sampler = Some(s);
                        }
                        Err(e) => tracing::error!("sample panicked: {e}"),
                    }
                }
            }
        }
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuTimes {
    idle: u64,
    total: u64,
}

fn cpu_times(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line
        .split_whitespace()
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    if v.len() < 5 {
        return None;
    }
    Some(CpuTimes {
        idle: v[3] + v[4],
        total: v.iter().take(8).sum(),
    })
}

fn cpu_usage(prev: CpuTimes, cur: CpuTimes) -> f64 {
    let total = cur.total.saturating_sub(prev.total);
    let idle = cur.idle.saturating_sub(prev.idle);
    if total == 0 {
        return 0.0;
    }
    ((total.saturating_sub(idle)) as f64 * 100.0 / total as f64).clamp(0.0, 100.0)
}

fn cpu_model(cpuinfo: &str) -> Option<String> {
    let raw = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))?
        .split_once(':')?
        .1;
    let mut m = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    for cut in [" @", " w/ Radeon", " with Radeon"] {
        if let Some(i) = m.find(cut) {
            m.truncate(i);
        }
    }
    let suffix = |w: &str| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "cpu"
                | "fpu"
                | "apu"
                | "processor"
                | "dual-core"
                | "quad-core"
                | "six-core"
                | "eight-core"
                | "ten-core"
        ) || w
            .to_ascii_lowercase()
            .strip_suffix("-core")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    if let Some((head, last)) = m.rsplit_once(' ')
        && suffix(last)
    {
        m = head.to_string();
    }
    Some(m)
}

struct MemInfo {
    total: u64,
    available: u64,
}

fn meminfo(s: &str) -> Option<MemInfo> {
    let field = |name: &str| -> Option<u64> {
        s.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|kib| kib * 1024)
    };
    let total = field("MemTotal")?;
    (total > 0).then_some(MemInfo {
        total,
        available: field("MemAvailable")?,
    })
}

fn read_trimmed(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn first_hwmon(dir: &Path) -> Option<PathBuf> {
    let mut hw: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    hw.sort();
    hw.into_iter().next()
}

fn cpu_temp() -> i32 {
    let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") else {
        return -1;
    };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for dir in dirs {
        let Some(name) = read_trimmed(&dir.join("name")) else {
            continue;
        };
        if !CPU_HWMON.contains(&name.as_str()) {
            continue;
        }
        let Ok(items) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut inputs: Vec<(u32, PathBuf)> = items
            .filter_map(|e| {
                let p = e.ok()?.path();
                let n = p
                    .file_name()?
                    .to_str()?
                    .strip_prefix("temp")?
                    .strip_suffix("_input")?
                    .parse()
                    .ok()?;
                Some((n, p))
            })
            .collect();
        inputs.sort();
        for (_, p) in inputs {
            if let Some(m) = read_trimmed(&p).and_then(|s| s.parse::<i32>().ok())
                && plausible_millicelsius(m)
            {
                return m / 1000;
            }
        }
    }
    -1
}

fn plausible_millicelsius(m: i32) -> bool {
    10_000 < m && m < 120_000
}

fn detect_gpus() -> Vec<Card> {
    let mut cards = Vec::new();

    if let Ok(entries) = std::fs::read_dir("/proc/driver/nvidia/gpus") {
        let mut pcis: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        pcis.sort();
        for pci in pcis {
            let name =
                std::fs::read_to_string(format!("/proc/driver/nvidia/gpus/{pci}/information"))
                    .ok()
                    .and_then(|s| nvidia_model(&s))
                    .unwrap_or_else(|| "NVIDIA GPU".into());
            cards.push(Card {
                name,
                source: Source::Nvidia {
                    power: PathBuf::from(format!(
                        "/sys/bus/pci/devices/{pci}/power/runtime_status"
                    )),
                    pci,
                },
            });
        }
    }

    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("card") && !n.contains('-'))
            .collect();
        names.sort();
        for card in names {
            let device = PathBuf::from(format!("/sys/class/drm/{card}/device"));
            let index = card.trim_start_matches("card");
            match read_trimmed(&device.join("vendor")).as_deref() {
                Some("0x1002") => cards.push(Card {
                    name: format!("AMD GPU {index}"),
                    source: Source::Amd { device },
                }),
                Some("0x8086") => cards.push(Card {
                    name: format!("Intel GPU {index}"),
                    source: Source::Intel,
                }),
                _ => {}
            }
        }
    }

    cards
}

fn nvidia_model(information: &str) -> Option<String> {
    information
        .lines()
        .find_map(|l| l.strip_prefix("Model:"))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

fn with_kinds(mounts: Vec<String>) -> Vec<(String, String)> {
    let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    mounts
        .into_iter()
        .map(|m| {
            let kind = mount_source(&info, &m)
                .and_then(|src| disk_kind(&src))
                .unwrap_or("unknown");
            (m, kind.to_string())
        })
        .collect()
}

fn mount_source(mountinfo: &str, mount: &str) -> Option<String> {
    mountinfo.lines().rev().find_map(|l| {
        let (pre, post) = l.split_once(" - ")?;
        let point = pre.split(' ').nth(4)?;
        let source = post.split(' ').nth(1)?;
        (unescape(point) == mount).then(|| unescape(source))
    })
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && let Some(oct) = b.get(i + 1..i + 4)
            && oct.iter().all(|d| (b'0'..=b'7').contains(d))
        {
            out.push(oct.iter().fold(0u8, |v, d| v.wrapping_mul(8) + (d - b'0')));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn disk_kind(source: &str) -> Option<&'static str> {
    let dev = std::fs::canonicalize(source).ok()?;
    let name = dev.strip_prefix("/dev").ok()?.to_str()?;
    let mut node = std::fs::canonicalize(format!("/sys/class/block/{name}")).ok()?;
    if node.join("partition").exists() {
        node.pop();
    }
    match read_trimmed(&node.join("queue/rotational"))?.as_str() {
        "1" => Some("hdd"),
        "0" => Some("ssd"),
        _ => None,
    }
}

fn disk_usage(mount: &str) -> Option<f64> {
    let path = CString::new(mount).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid NUL-terminated path and a correctly sized out-struct.
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(used_percent(
        st.f_blocks as u64,
        st.f_bavail as u64,
        st.f_frsize as u64,
    ))
}

fn used_percent(blocks: u64, avail: u64, frsize: u64) -> f64 {
    let total = blocks * frsize;
    if total == 0 {
        return 0.0;
    }
    (total.saturating_sub(avail * frsize)) as f64 * 100.0 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_times_read_the_aggregate_line_only() {
        let stat = "cpu  100 5 50 800 20 3 2 10 0 0\ncpu0 1 1 1 1 1 1 1 1 1 1\n";
        let t = cpu_times(stat).unwrap();
        assert_eq!(t.idle, 820);
        assert_eq!(t.total, 990);
        assert!(cpu_times("cpu0 1 2 3 4 5\n").is_none());
        assert!(cpu_times("cpu  1 2 3\n").is_none());
    }

    #[test]
    fn usage_is_busy_share_of_the_delta() {
        let a = CpuTimes {
            idle: 800,
            total: 1000,
        };
        let b = CpuTimes {
            idle: 875,
            total: 1100,
        };
        assert!((cpu_usage(a, b) - 25.0).abs() < 1e-9);
        assert_eq!(cpu_usage(a, a), 0.0);
        assert_eq!(cpu_usage(b, a), 0.0);
    }

    #[test]
    fn cpu_model_loses_clock_and_marketing_suffixes() {
        let m = |s: &str| cpu_model(&format!("processor\t: 0\nmodel name\t: {s}\n")).unwrap();
        assert_eq!(
            m("11th Gen Intel(R) Core(TM) i7-11800H @ 2.30GHz"),
            "11th Gen Intel(R) Core(TM) i7-11800H"
        );
        assert_eq!(
            m("AMD Ryzen 7 5800H with Radeon Graphics"),
            "AMD Ryzen 7 5800H"
        );
        assert_eq!(
            m("AMD Ryzen 9 7950X 16-Core Processor"),
            "AMD Ryzen 9 7950X 16-Core"
        );
        assert_eq!(m("AMD Ryzen 5 3600 6-Core"), "AMD Ryzen 5 3600");
        assert_eq!(m("Intel(R)   Xeon(R)  CPU"), "Intel(R) Xeon(R)");
        assert!(cpu_model("processor : 0\n").is_none());
    }

    #[test]
    fn meminfo_is_bytes() {
        let s = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    4000000 kB\n";
        let m = meminfo(s).unwrap();
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.available, 4_000_000 * 1024);
        assert!(meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n").is_none());
        assert!(meminfo("MemTotal: 10 kB\n").is_none());
    }

    #[test]
    fn mount_source_takes_the_topmost_and_unescapes() {
        let info = "\
22 1 259:3 /@ / rw,noatime shared:1 - btrfs /dev/nvme0n1p3 rw\n\
40 22 0:35 / /home rw shared:2 - tmpfs tmpfs rw\n\
41 22 259:4 / /home rw shared:3 - ext4 /dev/sda1 rw\n\
42 22 259:5 / /mnt/my\\040disk rw shared:4 - ext4 /dev/sdb1 rw\n";
        assert_eq!(mount_source(info, "/").as_deref(), Some("/dev/nvme0n1p3"));
        assert_eq!(mount_source(info, "/home").as_deref(), Some("/dev/sda1"));
        assert_eq!(
            mount_source(info, "/mnt/my disk").as_deref(),
            Some("/dev/sdb1")
        );
        assert!(mount_source(info, "/nope").is_none());
    }

    #[test]
    fn unescape_leaves_a_trailing_backslash_alone() {
        assert_eq!(unescape("a\\040b"), "a b");
        assert_eq!(unescape("a\\"), "a\\");
        assert_eq!(unescape("a\\04"), "a\\04");
    }

    #[test]
    fn disk_usage_counts_the_root_reserve_as_used() {
        assert!((used_percent(1000, 250, 4096) - 75.0).abs() < 1e-9);
        assert_eq!(used_percent(0, 0, 4096), 0.0);
    }

    #[test]
    fn temperature_window_rejects_sentinels() {
        assert!(plausible_millicelsius(45_000));
        assert!(!plausible_millicelsius(0));
        assert!(!plausible_millicelsius(-273_000));
        assert!(!plausible_millicelsius(127_000));
    }

    #[test]
    fn nvidia_model_comes_from_the_driver_table() {
        let info = "Model: \t\t NVIDIA GeForce RTX 3060 Laptop GPU\nIRQ:   16\n";
        assert_eq!(
            nvidia_model(info).as_deref(),
            Some("NVIDIA GeForce RTX 3060 Laptop GPU")
        );
        assert!(nvidia_model("IRQ: 16\n").is_none());
    }

    #[test]
    fn commands_parse() {
        let watch = Request {
            cmd: "watch".into(),
            args: serde_json::json!({"interval_ms": 2000, "disks": ["/", "/home"]}),
        };
        assert_eq!(
            Command::try_from(watch).unwrap(),
            Command::Watch {
                interval_ms: 2000,
                disks: vec!["/".into(), "/home".into()]
            }
        );
        let stop = Request {
            cmd: "stop".into(),
            args: serde_json::Value::Null,
        };
        assert_eq!(Command::try_from(stop).unwrap(), Command::Stop);
    }
}
