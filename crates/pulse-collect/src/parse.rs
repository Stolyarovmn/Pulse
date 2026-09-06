//! Парсеры файлов ядра.
//!
//! Правило для всего модуля: вход недоверенный. Любая функция возвращает
//! `Option`/пустой результат и никогда не паникует. Ошибка формата означает
//! «данных нет», а не «агент останавливается»: часть полей может отсутствовать
//! на старом ядре, а часть — быть намеренно испорчена локальным процессом.

/// Значение PSI: доли времени за окна и накопленное время задержки.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pressure {
    pub some_avg10: f64,
    pub some_avg60: f64,
    pub some_total_secs: f64,
    pub full_avg10: f64,
    pub full_avg60: f64,
    pub full_total_secs: f64,
}

/// Разбирает файл давления (`/proc/pressure/*`, `*.pressure` в cgroup).
///
/// Формат: `some avg10=0.00 avg60=0.00 avg300=0.00 total=0`.
/// `avg*` в файле выражены в процентах, наружу отдаются доли 0..1.
#[must_use]
pub fn parse_pressure(text: &str) -> Pressure {
    let mut out = Pressure::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(kind) = parts.next() else { continue };
        let mut avg10 = 0.0;
        let mut avg60 = 0.0;
        let mut total = 0.0;
        for field in parts {
            let mut kv = field.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let value = kv.next().and_then(|v| v.parse::<f64>().ok());
            match (key, value) {
                ("avg10", Some(v)) => avg10 = v / 100.0,
                ("avg60", Some(v)) => avg60 = v / 100.0,
                // total в микросекундах.
                ("total", Some(v)) => total = v / 1_000_000.0,
                _ => {}
            }
        }
        match kind {
            "some" => {
                out.some_avg10 = avg10;
                out.some_avg60 = avg60;
                out.some_total_secs = total;
            }
            "full" => {
                out.full_avg10 = avg10;
                out.full_avg60 = avg60;
                out.full_total_secs = total;
            }
            _ => {}
        }
    }
    out
}

/// Ищет значение в файле вида `ключ значение` (`meminfo`, `cpu.stat`, `memory.stat`).
///
/// Двоеточие после ключа игнорируется, поэтому одна функция подходит и для
/// `/proc/meminfo` (`MemTotal:       16316412 kB`), и для cgroup-файлов.
#[must_use]
pub fn field(text: &str, key: &str) -> Option<f64> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let name = parts.next()?.trim_end_matches(':');
        if name != key {
            continue;
        }
        let raw = parts.next()?;
        let value = raw.parse::<f64>().ok()?;
        // Единицы: `kB` в meminfo — единственный распространённый случай.
        let scale = match parts.next() {
            Some("kB") | Some("KB") => 1024.0,
            Some("mB") | Some("MB") => 1024.0 * 1024.0,
            _ => 1.0,
        };
        return Some(value * scale);
    }
    None
}

/// Все пары `ключ значение` файла.
#[must_use]
pub fn fields(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?.trim_end_matches(':').to_string();
            let value = parts.next()?.parse::<f64>().ok()?;
            Some((key, value))
        })
        .collect()
}

/// Значение лимита cgroup: `max` означает «не ограничено» и даёт 0.
#[must_use]
pub fn parse_limit(text: &str) -> f64 {
    let token = text.split_whitespace().next().unwrap_or("");
    if token == "max" {
        0.0
    } else {
        token.parse::<f64>().unwrap_or(0.0)
    }
}

/// Разбирает `cpu.max`: `<quota|max> <period>` → лимит в ядрах (0 если нет квоты).
#[must_use]
pub fn parse_cpu_max(text: &str) -> f64 {
    let mut parts = text.split_whitespace();
    let quota = parts.next().unwrap_or("max");
    let period = parts
        .next()
        .and_then(|p| p.parse::<f64>().ok())
        .unwrap_or(0.0);
    if quota == "max" || period <= 0.0 {
        return 0.0;
    }
    quota.parse::<f64>().map(|q| q / period).unwrap_or(0.0)
}

/// Суммарные значения `io.stat` cgroup по всем устройствам.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IoStat {
    pub rbytes: f64,
    pub wbytes: f64,
    pub rios: f64,
    pub wios: f64,
}

/// Разбирает `io.stat`: строка на устройство, поля `ключ=значение`.
#[must_use]
pub fn parse_io_stat(text: &str) -> IoStat {
    let mut out = IoStat::default();
    for line in text.lines() {
        for field in line.split_whitespace().skip(1) {
            let mut kv = field.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let Some(value) = kv.next().and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            match key {
                "rbytes" => out.rbytes += value,
                "wbytes" => out.wbytes += value,
                "rios" => out.rios += value,
                "wios" => out.wios += value,
                _ => {}
            }
        }
    }
    out
}

/// Идентификаторы устройств `major:minor`, по которым у cgroup есть трафик.
///
/// Нужны, чтобы связать потребителя с конкретным диском: без этого «диск
/// тормозит» и «этот сервис на нём лежит» остаются двумя независимыми фактами.
/// Устройства без единого ненулевого счётчика пропускаются: запись в `io.stat`
/// может существовать просто потому, что ядро когда-то видело устройство.
#[must_use]
pub fn io_stat_devices(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(device) = parts.next() else {
            continue;
        };
        // Идентификатор обязан иметь вид major:minor — иначе строка не наша.
        let mut halves = device.split(':');
        let valid = matches!((halves.next(), halves.next(), halves.next()), (Some(major), Some(minor), None)
            if !major.is_empty()
                && !minor.is_empty()
                && major.bytes().all(|b| b.is_ascii_digit())
                && minor.bytes().all(|b| b.is_ascii_digit()));
        if !valid {
            continue;
        }
        let has_traffic = parts.any(|field| {
            let mut kv = field.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let value = kv.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            matches!(key, "rbytes" | "wbytes" | "rios" | "wios") && value > 0.0
        });
        if has_traffic {
            out.push(device.to_string());
        }
    }
    out
}

/// Разобранная строка `/proc/<pid>/stat`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcStat {
    pub pid: i32,
    /// Имя из `comm`, как его записал сам процесс. Санитизация — выше по стеку.
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub nice: i64,
    pub num_threads: i64,
    pub start_ticks: u64,
    pub vsize_bytes: u64,
    pub rss_pages: u64,
    pub minor_faults: u64,
    pub major_faults: u64,
}

/// Разбирает `/proc/<pid>/stat`.
///
/// Тонкость, на которой ошибаются почти все наивные парсеры: `comm` заключён в
/// круглые скобки, но сам может содержать и пробелы, и скобки — например
/// `((sd-pam) x)`. Поэтому имя ограничивается **последней** закрывающей скобкой,
/// а не первой.
#[must_use]
pub fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid = text.get(..open)?.trim().parse::<i32>().ok()?;
    let comm = text.get(open + 1..close)?.to_string();
    let rest = text.get(close + 1..)?;

    // Поля после comm нумеруются с 3 по нумерации proc(5): rest[0] == поле 3.
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |index: usize| -> Option<u64> { f.get(index)?.parse::<u64>().ok() };
    let inum = |index: usize| -> Option<i64> { f.get(index)?.parse::<i64>().ok() };

    let state = f.first().and_then(|s| s.chars().next()).unwrap_or('?');
    Some(ProcStat {
        pid,
        comm,
        state,
        ppid: inum(1).unwrap_or(0) as i32,
        utime_ticks: num(11).unwrap_or(0),
        stime_ticks: num(12).unwrap_or(0),
        nice: inum(16).unwrap_or(0),
        num_threads: inum(17).unwrap_or(0),
        start_ticks: num(19).unwrap_or(0),
        vsize_bytes: num(20).unwrap_or(0),
        rss_pages: num(21).unwrap_or(0),
        minor_faults: num(7).unwrap_or(0),
        major_faults: num(9).unwrap_or(0),
    })
}

/// Агрегированные счётчики `/proc/stat` строки `cpu`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuTimes {
    pub user: f64,
    pub nice: f64,
    pub system: f64,
    pub idle: f64,
    pub iowait: f64,
    pub irq: f64,
    pub softirq: f64,
    pub steal: f64,
}

impl CpuTimes {
    /// Всё процессорное время, включая простой.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    /// Время, когда CPU был занят.
    #[must_use]
    pub fn busy(&self) -> f64 {
        self.total() - self.idle - self.iowait
    }
}

/// Разбирает суммарную строку `cpu` из `/proc/stat`.
#[must_use]
pub fn parse_cpu_times(text: &str) -> Option<CpuTimes> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() != Some("cpu") {
            continue;
        }
        let values: Vec<f64> = parts.filter_map(|v| v.parse::<f64>().ok()).collect();
        let get = |i: usize| values.get(i).copied().unwrap_or(0.0);
        return Some(CpuTimes {
            user: get(0),
            nice: get(1),
            system: get(2),
            idle: get(3),
            iowait: get(4),
            irq: get(5),
            softirq: get(6),
            steal: get(7),
        });
    }
    None
}

/// Счётчики одного блочного устройства из `/proc/diskstats`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskStat {
    /// Идентификатор ядра `major:minor`.
    pub device: String,
    pub name: String,
    pub read_ops: f64,
    pub read_sectors: f64,
    pub read_wait_ms: f64,
    pub write_ops: f64,
    pub write_sectors: f64,
    pub write_wait_ms: f64,
    pub inflight: f64,
    pub io_time_ms: f64,
    pub weighted_io_time_ms: f64,
}

/// Разбирает `/proc/diskstats`.
///
/// Поля: `major minor name reads merged sectors ms writes merged sectors ms
/// inflight io_ms weighted_ms ...` — начиная с ядра 4.18 есть ещё поля discard
/// и flush, они не нужны и игнорируются.
#[must_use]
pub fn parse_diskstats(text: &str) -> Vec<DiskStat> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 14 {
            continue;
        }
        let (Some(major), Some(minor), Some(name)) = (f.first(), f.get(1), f.get(2)) else {
            continue;
        };
        if !major.bytes().all(|b| b.is_ascii_digit()) || !minor.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let num = |index: usize| -> f64 {
            f.get(index)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        out.push(DiskStat {
            device: format!("{major}:{minor}"),
            name: (*name).to_string(),
            read_ops: num(3),
            read_sectors: num(5),
            read_wait_ms: num(6),
            write_ops: num(7),
            write_sectors: num(9),
            write_wait_ms: num(10),
            inflight: num(11),
            io_time_ms: num(12),
            weighted_io_time_ms: num(13),
        });
    }
    out
}

/// Счётчики сетевого интерфейса из `/proc/net/dev`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetStat {
    pub name: String,
    pub rx_bytes: f64,
    pub rx_packets: f64,
    pub rx_errors: f64,
    pub rx_drops: f64,
    pub tx_bytes: f64,
    pub tx_packets: f64,
    pub tx_errors: f64,
    pub tx_drops: f64,
}

/// Разбирает `/proc/net/dev`.
#[must_use]
pub fn parse_net_dev(text: &str) -> Vec<NetStat> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((raw_name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = raw_name.trim();
        if name.is_empty() {
            continue;
        }
        let values: Vec<f64> = rest
            .split_whitespace()
            .map(|v| v.parse::<f64>().unwrap_or(0.0))
            .collect();
        if values.len() < 16 {
            continue;
        }
        let get = |i: usize| values.get(i).copied().unwrap_or(0.0);
        out.push(NetStat {
            name: name.to_string(),
            rx_bytes: get(0),
            rx_packets: get(1),
            rx_errors: get(2),
            rx_drops: get(3),
            tx_bytes: get(8),
            tx_packets: get(9),
            tx_errors: get(10),
            tx_drops: get(11),
        });
    }
    out
}

/// Разбирает `/proc/loadavg` — три средних значения.
#[must_use]
pub fn parse_loadavg(text: &str) -> Option<(f64, f64, f64)> {
    let mut parts = text.split_whitespace();
    let one = parts.next()?.parse::<f64>().ok()?;
    let five = parts.next()?.parse::<f64>().ok()?;
    let fifteen = parts.next()?.parse::<f64>().ok()?;
    Some((one, five, fifteen))
}

/// Разбирает `/proc/sys/fs/file-nr`: `<выделено> <свободно> <максимум>`.
#[must_use]
pub fn parse_file_nr(text: &str) -> Option<(f64, f64)> {
    let values: Vec<f64> = text
        .split_whitespace()
        .filter_map(|v| v.parse::<f64>().ok())
        .collect();
    let allocated = values.first().copied()?;
    let free = values.get(1).copied().unwrap_or(0.0);
    let max = values.get(2).copied()?;
    Some((allocated - free, max))
}

/// Разбирает `/proc/net/snmp` и возвращает значение поля указанной секции.
#[must_use]
pub fn parse_snmp_field(text: &str, section: &str, field_name: &str) -> Option<f64> {
    let mut lines = text.lines();
    while let Some(header) = lines.next() {
        let Some((name, keys)) = header.split_once(':') else {
            continue;
        };
        if name.trim() != section {
            continue;
        }
        let values_line = lines.next()?;
        let (_, values) = values_line.split_once(':')?;
        let index = keys
            .split_whitespace()
            .position(|k| k.eq_ignore_ascii_case(field_name))?;
        return values
            .split_whitespace()
            .nth(index)
            .and_then(|v| v.parse::<f64>().ok());
    }
    None
}

/// Разбирает `/proc/net/sockstat`: значение поля секции (`TCP: inuse 12 ...`).
#[must_use]
pub fn parse_sockstat(text: &str, section: &str, field_name: &str) -> Option<f64> {
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != section {
            continue;
        }
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let position = tokens.iter().position(|t| *t == field_name)?;
        return tokens.get(position + 1).and_then(|v| v.parse::<f64>().ok());
    }
    None
}

/// Относительный путь cgroup из `/proc/<pid>/cgroup` (формат cgroup v2: `0::/path`).
#[must_use]
pub fn parse_proc_cgroup(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        // Единая иерархия v2: идентификатор 0 и пустой список контроллеров.
        if hierarchy == "0" && controllers.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}

/// Значение из `/proc/<pid>/limits` по имени лимита («Max open files»).
#[must_use]
pub fn parse_limit_row(text: &str, name: &str) -> Option<f64> {
    for line in text.lines() {
        if !line.starts_with(name) {
            continue;
        }
        let rest = line.get(name.len()..)?;
        let token = rest.split_whitespace().next()?;
        if token.eq_ignore_ascii_case("unlimited") {
            return Some(0.0);
        }
        return token.parse::<f64>().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_percentages_become_ratios() {
        let text = "some avg10=12.34 avg60=5.00 avg300=1.00 total=1234567\n\
                    full avg10=3.00 avg60=1.00 avg300=0.50 total=7654321\n";
        let p = parse_pressure(text);
        assert!((p.some_avg10 - 0.1234).abs() < 1e-9);
        assert!((p.full_avg10 - 0.03).abs() < 1e-9);
        assert!((p.some_total_secs - 1.234567).abs() < 1e-9);
    }

    #[test]
    fn pressure_of_garbage_is_zero_not_panic() {
        let p = parse_pressure("garbage\nsome avg10=abc total=\n\n");
        assert_eq!(p, Pressure::default());
    }

    #[test]
    fn meminfo_units_are_applied() {
        let text = "MemTotal:       16316412 kB\nMemAvailable:    8158206 kB\n";
        assert_eq!(field(text, "MemTotal"), Some(16_316_412.0 * 1024.0));
        assert_eq!(field(text, "Nope"), None);
    }

    #[test]
    fn cgroup_limits_treat_max_as_unlimited() {
        assert_eq!(parse_limit("max\n"), 0.0);
        assert_eq!(parse_limit("2147483648\n"), 2_147_483_648.0);
        assert_eq!(parse_cpu_max("max 100000\n"), 0.0);
        assert!((parse_cpu_max("800000 100000\n") - 8.0).abs() < 1e-12);
        assert_eq!(parse_cpu_max("garbage\n"), 0.0);
        assert_eq!(parse_cpu_max("100000 0\n"), 0.0);
    }

    #[test]
    fn io_stat_sums_devices() {
        let text = "8:0 rbytes=1000 wbytes=2000 rios=10 wios=20\n\
                    8:16 rbytes=500 wbytes=100 rios=5 wios=1\n";
        let io = parse_io_stat(text);
        assert!((io.rbytes - 1500.0).abs() < 1e-12);
        assert!((io.wios - 21.0).abs() < 1e-12);
    }

    #[test]
    fn io_stat_devices_lists_only_devices_with_traffic() {
        let text = "8:0 rbytes=1000 wbytes=0 rios=10 wios=0\n\
                    8:16 rbytes=0 wbytes=0 rios=0 wios=0\n\
                    253:1 wbytes=4096\n\
                    мусор rbytes=5\n";
        assert_eq!(
            io_stat_devices(text),
            vec!["8:0".to_string(), "253:1".to_string()]
        );
        assert!(io_stat_devices("").is_empty());
    }

    #[test]
    fn proc_stat_handles_parentheses_and_spaces_in_comm() {
        let text = "1234 ((sd-pam) x) S 1 1234 1234 0 -1 4194560 100 0 200 0 \
                    11 22 0 0 20 0 3 0 987654 123456789 4096 18446744073709551615";
        let stat = parse_proc_stat(text).expect("должно разобраться");
        assert_eq!(stat.pid, 1234);
        assert_eq!(stat.comm, "(sd-pam) x");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.utime_ticks, 11);
        assert_eq!(stat.stime_ticks, 22);
        assert_eq!(stat.num_threads, 3);
        assert_eq!(stat.start_ticks, 987_654);
        assert_eq!(stat.vsize_bytes, 123_456_789);
        assert_eq!(stat.rss_pages, 4096);
    }

    #[test]
    fn truncated_proc_stat_is_partial_not_panic() {
        let stat = parse_proc_stat("42 (nginx) R 1").expect("частичный разбор допустим");
        assert_eq!(stat.pid, 42);
        assert_eq!(stat.comm, "nginx");
        assert_eq!(stat.utime_ticks, 0);
    }

    #[test]
    fn malformed_proc_stat_returns_none() {
        assert!(parse_proc_stat("").is_none());
        assert!(parse_proc_stat("no parens here").is_none());
        assert!(parse_proc_stat("abc (x) R 1").is_none());
        assert!(parse_proc_stat("1 ) ( R").is_none());
    }

    #[test]
    fn cpu_times_are_parsed_and_derived() {
        let text = "cpu  100 20 30 1000 40 5 5 10 0 0\ncpu0 1 2 3 4 5 6 7 8\n";
        let times = parse_cpu_times(text).expect("строка cpu есть");
        assert!((times.user - 100.0).abs() < 1e-12);
        assert!((times.steal - 10.0).abs() < 1e-12);
        assert!((times.total() - 1210.0).abs() < 1e-12);
        assert!((times.busy() - 170.0).abs() < 1e-12);
    }

    #[test]
    fn diskstats_are_parsed() {
        let text = "   8       0 sda 1000 0 20000 500 2000 0 40000 900 0 1500 1400 0 0 0 0\n\
                    ignored line\n";
        let disks = parse_diskstats(text);
        assert_eq!(disks.len(), 1);
        let sda = disks.first().expect("устройство");
        assert_eq!(sda.name, "sda");
        assert!((sda.read_sectors - 20_000.0).abs() < 1e-12);
        assert!((sda.io_time_ms - 1_500.0).abs() < 1e-12);
        assert!((sda.weighted_io_time_ms - 1_400.0).abs() < 1e-12);
    }

    #[test]
    fn net_dev_is_parsed_with_alignment_variants() {
        let text = "Inter-|   Receive                                                |  Transmit\n\
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n\
  eth0:2000000 2000 1 2 0 0 0 0 3000000 3000 3 4 0 0 0 0\n";
        let stats = parse_net_dev(text);
        let eth0 = stats.iter().find(|s| s.name == "eth0").expect("eth0");
        assert!((eth0.rx_bytes - 2_000_000.0).abs() < 1e-12);
        assert!((eth0.tx_packets - 3_000.0).abs() < 1e-12);
        assert!((eth0.rx_drops - 2.0).abs() < 1e-12);
    }

    #[test]
    fn loadavg_and_file_nr_are_parsed() {
        assert_eq!(
            parse_loadavg("1.25 1.32 1.18 2/1234 5678\n"),
            Some((1.25, 1.32, 1.18))
        );
        assert_eq!(
            parse_file_nr("1216 0 9223372036854775807\n"),
            Some((1216.0, 9.223372036854776e18))
        );
        assert_eq!(parse_file_nr("garbage"), None);
    }

    #[test]
    fn snmp_and_sockstat_fields_are_found() {
        let snmp = "Tcp: RtoAlgorithm RtoMin RetransSegs InSegs\nTcp: 1 200 4242 999\n";
        assert_eq!(parse_snmp_field(snmp, "Tcp", "RetransSegs"), Some(4242.0));
        assert_eq!(parse_snmp_field(snmp, "Udp", "RetransSegs"), None);

        let sockstat = "sockets: used 500\nTCP: inuse 42 orphan 0 tw 1 alloc 50 mem 3\n";
        assert_eq!(parse_sockstat(sockstat, "TCP", "inuse"), Some(42.0));
        assert_eq!(parse_sockstat(sockstat, "TCP", "nope"), None);
    }

    #[test]
    fn proc_cgroup_v2_path_is_extracted() {
        let text = "0::/system.slice/nginx.service\n";
        assert_eq!(
            parse_proc_cgroup(text),
            Some("/system.slice/nginx.service".to_string())
        );
        // Гибридная иерархия v1 не поддерживается сознательно.
        assert!(parse_proc_cgroup("1:cpu:/foo\n").is_none());
    }

    #[test]
    fn limit_row_handles_unlimited() {
        let text = "Limit                     Soft Limit           Hard Limit           Units\n\
                    Max open files            1024                 4096                 files\n";
        assert_eq!(parse_limit_row(text, "Max open files"), Some(1024.0));
        assert_eq!(
            parse_limit_row("Max open files            unlimited", "Max open files"),
            Some(0.0)
        );
        assert_eq!(parse_limit_row(text, "Max nope"), None);
    }
}
