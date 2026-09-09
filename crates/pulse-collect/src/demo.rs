//! Демонстрационный источник файловой системы.
//!
//! Существует по одной причине: на здоровом хосте показать диагностику
//! нечем. Правила, Timeline и A/B diff — самое ценное, что есть в Pulse,
//! но кадр на исправной машине честно говорит «проблем нет», и ни принять
//! работу, ни показать её невозможно.
//!
//! Ключевое свойство: демо подменяет **только** [`FsSource`], то есть
//! нижний слой чтения файлов. Дальше идёт настоящий конвейер — те же
//! коллекторы, тот же граф сущностей, те же правила с гистерезисом, та же
//! история. Демо доказывает работу движка, а не рисует картинку поверх него.
//! Поэтому же оно не имеет права быть незаметным: режим обязан быть назван
//! в каждом кадре, и за это отвечает интерфейс.
//!
//! Сценарий (такт — один шаг конвейера, по умолчанию секунда):
//!
//! | Такты | Что происходит |
//! |---|---|
//! | 0–29 | покой: нагрузка низкая, память втрое ниже лимита |
//! | 30–74 | деградация `api.service`: память идёт к лимиту, cgroup упирается в квоту CPU, растёт давление |
//! | 75–149 | восстановление: значения возвращаются к покою |
//!
//! Пороги взяты не на глаз: `memory_util_warn` 0.85 и `memory_util_crit`
//! 0.95, `throttle_warn` 0.10, `psi_*` — из `Rules::default`. Сценарий
//! обязан пересекать их с запасом, иначе демо не покажет ни открытия
//! проблемы, ни её закрытия по гистерезису.
//!
//! Длина фазы покоя выбрана не произвольно: она обязана превышать
//! `clear_after_ticks` (15 такта в `Rules::default`), иначе проблема
//! не успевает закрыться до следующей деградации и висит вечно —
//! это поймал `demo_scenario_opens_and_closes_a_problem_through_real_rules`.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::fs::{FixtureFs, FsSource, FsUsage};

/// Длина цикла сценария в тактах.
pub const CYCLE_TICKS: u64 = 150;

/// Такт, на котором начинается деградация.
pub const DEGRADE_FROM: u64 = 30;

/// Такт, на котором начинается восстановление.
pub const RECOVER_FROM: u64 = 75;

/// Источник, отдающий сцену демонстрации вместо реальных procfs и sysfs.
#[derive(Debug)]
pub struct DemoFs {
    started: Instant,
    /// Длительность одного такта сценария.
    step: Duration,
    /// Смещение начала сценария в тактах.
    ///
    /// Демо начинается не с нуля: первые такты покоя дают ровную линию,
    /// и оператор успевает решить, что кадр мёртвый, до начала деградации.
    /// Стартуем за несколько тактов до неё — первый же кадр уже движется.
    offset: u64,
    /// Дерево пересобирается только при смене такта: коллекторы делают
    /// десятки чтений за такт, и пересборка на каждое чтение сделала бы
    /// демо дороже реального сбора.
    scene: Mutex<(u64, FixtureFs)>,
    /// Такт зафиксирован: источник не идёт по часам.
    ///
    /// Нужен приёмке: тест, берущий такт из часов, зависел бы от скорости
    /// прогона и падал бы на медленной машине.
    frozen: Option<u64>,
}

impl DemoFs {
    /// Создаёт источник с указанной длительностью такта.
    #[must_use]
    pub fn new(step: Duration) -> Self {
        let step = step.max(Duration::from_millis(50));
        let offset = DEGRADE_FROM.saturating_sub(5);
        DemoFs {
            started: Instant::now(),
            step,
            scene: Mutex::new((offset, scene(offset))),
            frozen: None,
            offset,
        }
    }

    /// Источник, замороженный на указанном такте.
    #[must_use]
    pub fn at_tick(tick: u64) -> Self {
        DemoFs {
            started: Instant::now(),
            step: Duration::from_millis(1_000),
            scene: Mutex::new((tick, scene(tick))),
            frozen: Some(tick),
            offset: 0,
        }
    }

    /// Номер текущего такта сценария.
    #[must_use]
    pub fn tick(&self) -> u64 {
        if let Some(tick) = self.frozen {
            return tick;
        }
        let elapsed = self.started.elapsed().as_millis();
        let step = self.step.as_millis().max(1);
        self.offset
            .saturating_add(u64::try_from(elapsed / step).unwrap_or(u64::MAX))
    }

    /// Человеческое имя текущей фазы: интерфейс называет его оператору.
    #[must_use]
    pub fn phase(&self) -> &'static str {
        match self.tick() % CYCLE_TICKS {
            tick if tick < DEGRADE_FROM => "покой",
            tick if tick < RECOVER_FROM => "деградация api.service",
            _ => "восстановление",
        }
    }

    /// Выполняет чтение на дереве текущего такта.
    ///
    /// Замок отравлен только паникой внутри этой же функции; данные при
    /// этом остаются согласованными, поэтому продолжать безопаснее, чем
    /// уронить сбор.
    fn with_scene<T>(&self, f: impl FnOnce(&FixtureFs) -> T) -> T {
        let tick = self.tick();
        let mut guard = match self.scene.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.0 != tick {
            *guard = (tick, scene(tick));
        }
        f(&guard.1)
    }
}

impl FsSource for DemoFs {
    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>> {
        self.with_scene(|scene| scene.read(path, cap))
    }

    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
        self.with_scene(|scene| scene.scan_dir(path, visit))
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.with_scene(|scene| scene.read_link(path))
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.with_scene(|scene| scene.inode(path))
    }

    fn statfs(&self, path: &Path) -> io::Result<FsUsage> {
        self.with_scene(|scene| scene.statfs(path))
    }
}

/// Доля деградации в этом такте: 0.0 — покой, 1.0 — худший момент.
///
/// Плавный подъём, а не ступень: правило обязано открыться после
/// `enter_after_ticks` подряд, и ступенчатый переход не показал бы,
/// что гистерезис работает.
fn severity(tick: u64) -> f64 {
    let phase = tick % CYCLE_TICKS;
    if phase < DEGRADE_FROM {
        return 0.0;
    }
    if phase < RECOVER_FROM {
        let span = f64::from(u32::try_from(RECOVER_FROM - DEGRADE_FROM).unwrap_or(1));
        let position = f64::from(u32::try_from(phase - DEGRADE_FROM).unwrap_or(0));
        return (position / span).min(1.0);
    }
    let span = f64::from(u32::try_from(CYCLE_TICKS - RECOVER_FROM).unwrap_or(1));
    let position = f64::from(u32::try_from(phase - RECOVER_FROM).unwrap_or(0));
    (1.0 - position / span).max(0.0)
}

/// Линейная интерполяция между значениями покоя и худшего момента.
fn between(calm: f64, worst: f64, weight: f64) -> f64 {
    calm + (worst - calm) * weight
}

/// Накопленный счётчик: сумма приращений по всем прошедшим тактам.
///
/// Считать счётчик как «номер такта × текущая нагрузка» нельзя: при спаде
/// нагрузки произведение убывает, ядро же счётчики только увеличивает,
/// а производные метрики из убывающего счётчика выходят отрицательными.
/// Это поймал тест `counters_never_go_backwards`.
fn accumulated(tick: u64, per_tick: impl Fn(f64) -> f64) -> u64 {
    let mut total = 0.0_f64;
    for step in 0..=tick {
        total += per_tick(severity(step)).max(0.0);
    }
    total as u64
}

/// Строка `/proc/<pid>/stat` в том же порядке полей, что у ядра.
fn stat_line(pid: i32, comm: &str, ticks: u64, start_ticks: u64, rss_pages: u64) -> String {
    format!(
        "{pid} ({comm}) S 1 {pid} {pid} 0 -1 0 100 0 200 0 {ticks} {ticks} 0 0 20 0 4 0 \
         {start_ticks} 4194304 {rss_pages}"
    )
}

/// Дерево procfs, sysfs и cgroupfs для указанного такта.
#[allow(clippy::too_many_lines)]
fn scene(tick: u64) -> FixtureFs {
    let weight = severity(tick);
    let elapsed = tick + 1;

    // Хост: время в состоянии `iowait` и `system` растёт быстрее в деградации,
    // поэтому производные метрики CPU меняются, а не стоят на месте.
    let busy = 400 + accumulated(tick, |weight| between(10.0, 70.0, weight));
    let idle = 400 + accumulated(tick, |weight| between(90.0, 30.0, weight));
    let psi_cpu = between(3.0, 62.0, weight);
    let psi_memory = between(0.5, 31.0, weight);
    let psi_io = between(1.0, 38.0, weight);

    // Память хоста: свободная убывает, swap начинает расходоваться.
    let mem_total = 8_000_000_u64;
    let mem_free = between(3_000_000.0, 260_000.0, weight) as u64;
    let swap_total = 2_000_000_u64;
    let swap_free = between(2_000_000.0, 700_000.0, weight) as u64;

    // Сервис в cgroup: память идёт к своему лимиту, а CPU упирается в квоту.
    let mem_max = 209_715_200_u64;
    let mem_current = between(62_000_000.0, 205_000_000.0, weight) as u64;
    let cpu_usage_usec = accumulated(tick, |weight| between(40_000.0, 190_000.0, weight));
    let nr_periods = elapsed * 10;
    let nr_throttled = accumulated(tick, |weight| between(0.0, 4.2, weight));
    let throttled_usec = nr_throttled * 5_000;

    // Диск и сеть: счётчики накапливаются, а не масштабируются нагрузкой.
    let read_ios = elapsed * 120;
    let write_ios = accumulated(tick, |weight| between(80.0, 980.0, weight));
    let io_ms = accumulated(tick, |weight| between(40.0, 1_640.0, weight));
    let rx_bytes = elapsed * 900_000;
    let tx_bytes = elapsed * 300_000;

    let fd_open = between(1_200.0, 58_000.0, weight) as u64;

    FixtureFs::new()
        // ── хост ────────────────────────────────────────────────────────────
        .file(
            "/proc/stat",
            &format!(
                "cpu  {busy} 100 {busy} {idle} 200 0 0 0 0 0\n\
                 cpu0 {busy} 50 {busy} {idle} 100 0 0 0\n\
                 ctxt {}\n\
                 processes {}\n\
                 procs_running {}\n\
                 procs_blocked {}\n",
                elapsed * 4_000,
                elapsed * 40,
                1 + (weight * 12.0) as u64,
                (weight * 4.0) as u64,
            ),
        )
        .file(
            "/proc/meminfo",
            &format!(
                "MemTotal:       {mem_total} kB\n\
                 MemFree:        {mem_free} kB\n\
                 MemAvailable:   {} kB\n\
                 Buffers:         100000 kB\n\
                 Cached:         1200000 kB\n\
                 SwapTotal:      {swap_total} kB\n\
                 SwapFree:       {swap_free} kB\n",
                mem_free + 400_000,
            ),
        )
        .file(
            "/proc/pressure/cpu",
            &format!("some avg10={psi_cpu:.2} avg60={psi_cpu:.2} avg300=1.00 total=5000000\n"),
        )
        .file(
            "/proc/pressure/memory",
            &format!(
                "some avg10={psi_memory:.2} avg60={psi_memory:.2} avg300=1.00 total=4000000\n\
                 full avg10={:.2} avg60={:.2} avg300=0.50 total=2000000\n",
                psi_memory / 2.0,
                psi_memory / 2.0,
            ),
        )
        .file(
            "/proc/pressure/io",
            &format!(
                "some avg10={psi_io:.2} avg60={psi_io:.2} avg300=2.00 total=9000000\n\
                 full avg10={:.2} avg60={:.2} avg300=1.00 total=4000000\n",
                psi_io / 2.0,
                psi_io / 2.0,
            ),
        )
        .file(
            "/proc/loadavg",
            &format!("{:.2} 1.32 1.18 2/1234 5678\n", between(0.4, 9.5, weight)),
        )
        .file("/proc/uptime", &format!("{}.00 987654.32\n", 3_600 + elapsed))
        .file("/proc/sys/fs/file-nr", &format!("{fd_open} 0 65536\n"))
        .file("/proc/sys/kernel/hostname", "demo-host\n")
        .file(
            "/proc/sys/kernel/random/boot_id",
            "00000000-0000-4000-8000-000000000001\n",
        )
        .file("/proc/net/sockstat", "TCP: inuse 42 orphan 0 tw 1\n")
        .file(
            "/proc/net/snmp",
            &format!(
                "Tcp: RtoAlgorithm RtoMin RetransSegs\nTcp: 1 200 {}\n",
                elapsed * (1 + (weight * 90.0) as u64)
            ),
        )
        .file(
            "/proc/vmstat",
            &format!("nr_free_pages 1000\noom_kill {}\n", (weight * 3.0) as u64),
        )
        // ── диск, сеть, монтирования ────────────────────────────────────────
        .file(
            "/proc/diskstats",
            &format!(
                "   8       0 sda {read_ios} 0 {} {io_ms} {write_ios} 0 {} {io_ms} 0 {io_ms} {io_ms}\n",
                read_ios * 8,
                write_ios * 8,
            ),
        )
        .file(
            "/proc/net/dev",
            &format!(
                "Inter-|   Receive                                                |  Transmit\n\
                 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
                   eth0: {rx_bytes} {} 0 0 0 0 0 0 {tx_bytes} {} 0 0 0 0 0 0\n\
                     lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n",
                elapsed * 900,
                elapsed * 300,
            ),
        )
        .file(
            "/proc/mounts",
            "/dev/sda1 / ext4 rw,relatime 0 0\nproc /proc proc rw 0 0\n",
        )
        .mount("/", 100_000_000_000, 20_000_000_000, 18_000_000_000)
        // ── cgroup v2: корень, срез и два сервиса ───────────────────────────
        .inode("/sys/fs/cgroup", 1)
        .inode("/sys/fs/cgroup/system.slice", 2)
        .inode("/sys/fs/cgroup/system.slice/api.service", 3)
        .inode("/sys/fs/cgroup/system.slice/db.service", 4)
        .file("/sys/fs/cgroup/cgroup.controllers", "cpu memory io pids\n")
        .file(
            "/sys/fs/cgroup/cpu.stat",
            &format!("usage_usec {}\nnr_periods 0\nnr_throttled 0\n", elapsed * 300_000),
        )
        .file("/sys/fs/cgroup/memory.current", "3000000000\n")
        .file("/sys/fs/cgroup/memory.max", "max\n")
        .file("/sys/fs/cgroup/cgroup.procs", "")
        .file(
            "/sys/fs/cgroup/system.slice/cpu.stat",
            &format!(
                "usage_usec {}\nnr_periods {nr_periods}\nnr_throttled {nr_throttled}\nthrottled_usec {throttled_usec}\n",
                elapsed * 250_000
            ),
        )
        .file("/sys/fs/cgroup/system.slice/memory.current", "2500000000\n")
        .file("/sys/fs/cgroup/system.slice/memory.max", "max\n")
        .file("/sys/fs/cgroup/system.slice/cgroup.procs", "")
        .file(
            "/sys/fs/cgroup/system.slice/api.service/cpu.stat",
            &format!(
                "usage_usec {cpu_usage_usec}\nnr_periods {nr_periods}\nnr_throttled {nr_throttled}\nthrottled_usec {throttled_usec}\n"
            ),
        )
        .file("/sys/fs/cgroup/system.slice/api.service/cpu.max", "80000 100000\n")
        .file(
            "/sys/fs/cgroup/system.slice/api.service/memory.current",
            &format!("{mem_current}\n"),
        )
        .file(
            "/sys/fs/cgroup/system.slice/api.service/memory.max",
            &format!("{mem_max}\n"),
        )
        .file(
            "/sys/fs/cgroup/system.slice/api.service/memory.stat",
            &format!("anon {}\nfile {}\n", mem_current / 2, mem_current / 2),
        )
        .file(
            "/sys/fs/cgroup/system.slice/api.service/io.stat",
            &format!("8:0 rbytes={} wbytes={} rios={read_ios} wios={write_ios}\n", read_ios * 4_096, write_ios * 4_096),
        )
        .file("/sys/fs/cgroup/system.slice/api.service/pids.current", "42\n")
        .file("/sys/fs/cgroup/system.slice/api.service/pids.max", "100\n")
        .file("/sys/fs/cgroup/system.slice/api.service/cgroup.procs", "100\n101\n")
        .file(
            "/sys/fs/cgroup/system.slice/db.service/cpu.stat",
            &format!("usage_usec {}\nnr_periods {nr_periods}\nnr_throttled 0\nthrottled_usec 0\n", elapsed * 30_000),
        )
        .file("/sys/fs/cgroup/system.slice/db.service/memory.current", "402653184\n")
        .file("/sys/fs/cgroup/system.slice/db.service/memory.max", "2147483648\n")
        .file("/sys/fs/cgroup/system.slice/db.service/pids.current", "12\n")
        .file("/sys/fs/cgroup/system.slice/db.service/cgroup.procs", "200\n")
        // ── процессы ────────────────────────────────────────────────────────
        .inode("/proc", 1)
        .file("/proc/1/stat", &stat_line(1, "systemd", 500, 100, 3_000))
        .file("/proc/1/status", "Name:\tsystemd\nUid:\t0\t0\t0\t0\nThreads:\t1\n")
        .file("/proc/1/cgroup", "0::/init.scope\n")
        .file("/proc/1/statm", "30000 3000 1000 100 0 2000 0\n")
        .file(
            "/proc/100/stat",
            &stat_line(100, "api", elapsed * (5 + (weight * 55.0) as u64), 5_000, mem_current / 4_096),
        )
        .file(
            "/proc/100/status",
            &format!(
                "Name:\tapi\nUid:\t1000\t1000\t1000\t1000\nThreads:\t{}\nVmRSS:\t{} kB\n",
                4 + (weight * 20.0) as u64,
                mem_current / 1_024,
            ),
        )
        .file("/proc/100/statm", &format!("{} {} 1000 100 0 2000 0\n", mem_current / 4_096, mem_current / 4_096))
        .file("/proc/100/cgroup", "0::/system.slice/api.service\n")
        .file("/proc/100/cmdline", "api-server\0--port\08080\0")
        .file("/proc/100/oom_score", "17\n")
        .file(
            "/proc/100/io",
            &format!("read_bytes: {}\nwrite_bytes: {}\n", read_ios * 4_096, write_ios * 4_096),
        )
        .link("/proc/100/exe", "/usr/local/bin/api-server")
        .link("/proc/100/cwd", "/srv/api")
        .inode("/proc/100/fd/0", 10)
        .inode("/proc/100/fd/1", 11)
        .inode("/proc/100/fd/2", 12)
        .file("/proc/101/stat", &stat_line(101, "api-worker", elapsed * 3, 5_100, 20_000))
        .file("/proc/101/status", "Name:\tapi-worker\nUid:\t1000\t1000\t1000\t1000\nThreads:\t2\n")
        .file("/proc/101/cgroup", "0::/system.slice/api.service\n")
        .file("/proc/101/statm", "20000 4000 1000 100 0 2000 0\n")
        .file("/proc/200/stat", &stat_line(200, "postgres", elapsed * 4, 6_000, 90_000))
        .file("/proc/200/status", "Name:\tpostgres\nUid:\t70\t70\t70\t70\nThreads:\t8\n")
        .file("/proc/200/cgroup", "0::/system.slice/db.service\n")
        .file("/proc/200/statm", "90000 40000 1000 100 0 2000 0\n")
        // Собственные метрики агента: без них экран Overview честно пуст.
        .file("/proc/self/stat", &stat_line(999, "pulse", elapsed * 2, 7_000, 12_000))
        .file("/proc/self/statm", "12000 3000 1000 100 0 2000 0\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Сценарий обязан проходить фазы, а не стоять на месте: иначе демо
    /// не покажет ни открытия проблемы, ни её закрытия.
    #[test]
    fn scenario_walks_calm_degrade_and_recover() {
        assert!((severity(0) - 0.0).abs() < f64::EPSILON, "начало — покой");
        assert!(severity(DEGRADE_FROM + 20) > 0.4, "середина — деградация");
        assert!(
            severity(RECOVER_FROM + 1) > severity(CYCLE_TICKS - 2),
            "в фазе восстановления давление спадает"
        );
        assert!(
            (severity(CYCLE_TICKS) - 0.0).abs() < f64::EPSILON,
            "цикл замыкается в покой"
        );
    }

    /// Худший момент обязан пересекать пороги правил, иначе демо покажет
    /// «проблем нет» — то самое, из-за чего оно и понадобилось.
    #[test]
    fn worst_moment_crosses_rule_thresholds() {
        let rules = pulse_core::config::Rules::default();
        let worst = scene(RECOVER_FROM - 1);

        let current: f64 = read_number(
            &worst,
            "/sys/fs/cgroup/system.slice/api.service/memory.current",
        );
        let max: f64 = read_number(&worst, "/sys/fs/cgroup/system.slice/api.service/memory.max");
        assert!(
            current / max > rules.memory_util_crit,
            "занятость памяти {:.3} обязана превышать критический порог {}",
            current / max,
            rules.memory_util_crit
        );

        // Правило считает долю троттлинга по приращению между тактами
        // (`cgroup.rs`: `d_throttled / d_periods`), поэтому и проверять
        // надо приращение: отношение накопленных сумм усреднено по всей
        // истории и порога не пересечёт никогда.
        let previous = scene(RECOVER_FROM - 2);
        let stat_now = read_text(&worst, "/sys/fs/cgroup/system.slice/api.service/cpu.stat");
        let stat_before = read_text(
            &previous,
            "/sys/fs/cgroup/system.slice/api.service/cpu.stat",
        );
        let d_periods = field(&stat_now, "nr_periods") - field(&stat_before, "nr_periods");
        let d_throttled = field(&stat_now, "nr_throttled") - field(&stat_before, "nr_throttled");
        assert!(d_periods > 0.0, "периоды квоты обязаны идти вперёд");
        assert!(
            d_throttled / d_periods > rules.throttle_warn,
            "доля троттлинга {:.3} обязана превышать порог {}",
            d_throttled / d_periods,
            rules.throttle_warn
        );

        let psi = read_text(&worst, "/proc/pressure/cpu");
        let value: f64 = psi
            .split("avg10=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .expect("avg10 в /proc/pressure/cpu");
        assert!(
            value / 100.0 > rules.psi_cpu_crit,
            "давление CPU {value} обязано превышать критический порог"
        );
    }

    /// Монотонные счётчики обязаны только расти: убывание даёт отрицательные
    /// производные метрики, и кадр показал бы бессмыслицу.
    #[test]
    fn counters_never_go_backwards() {
        for path in [
            "/proc/diskstats",
            "/proc/net/dev",
            "/sys/fs/cgroup/system.slice/api.service/cpu.stat",
        ] {
            let mut previous = f64::MIN;
            for tick in 0..(CYCLE_TICKS * 2) {
                let text = read_text(&scene(tick), path);
                let sum: f64 = text
                    .split(|c: char| !c.is_ascii_digit())
                    .filter_map(|token| token.parse::<f64>().ok())
                    .sum();
                assert!(
                    sum >= previous,
                    "{path}: сумма счётчиков упала на такте {tick}"
                );
                previous = sum;
            }
        }
    }

    /// Демо начинается у порога деградации, а не с нуля.
    ///
    /// Дефект восприятия с живого прогона: тридцать тактов покоя в начале
    /// давали ровную линию, и кадр выглядел мёртвым до первого срабатывания
    /// правила. Фаза при этом обязана называться человеческим словом:
    /// оператор должен видеть, что именно он сейчас наблюдает.
    #[test]
    fn demo_starts_just_before_degradation() {
        let demo = DemoFs::new(Duration::from_secs(3_600));
        assert_eq!(demo.tick(), DEGRADE_FROM - 5, "старт у порога деградации");
        assert_eq!(demo.phase(), "покой");

        let degrading = DemoFs::at_tick(DEGRADE_FROM + 10);
        assert_eq!(degrading.phase(), "деградация api.service");
        let recovering = DemoFs::at_tick(RECOVER_FROM + 10);
        assert_eq!(recovering.phase(), "восстановление");
    }

    fn read_text(scene: &FixtureFs, path: &str) -> String {
        let bytes = scene
            .read(Path::new(path), 64 * 1024)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn read_number(scene: &FixtureFs, path: &str) -> f64 {
        read_text(scene, path).trim().parse().expect("число")
    }

    fn field(text: &str, name: &str) -> f64 {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().parse().ok())
            .unwrap_or_else(|| panic!("поле {name} не найдено"))
    }
}
