//! Реестр метрик.
//!
//! Все метрики объявлены статически: идентификатор — `u16`, а не строка. Это даёт
//! дешёвый `SeriesKey`, исключает опечатки в именах и делает cardinality
//! контролируемой на входе, а не в экспортере.
//!
//! Добавление метрики: одна строка в макросе `metrics!`. Идентификаторы
//! распределены по диапазонам, чтобы параллельные правки не конфликтовали:
//!
//! | Диапазон | Область |
//! |---|---|
//! | 1..=49 | host |
//! | 50..=69 | disk |
//! | 70..=89 | network interface |
//! | 90..=149 | cgroup (и производные владельцы: unit/container/pod) |
//! | 150..=189 | process |
//! | 190..=219 | самонаблюдение агента |

use std::fmt;

use serde::{Deserialize, Serialize};

/// Идентификатор метрики. `0` не используется — резерв для «нет метрики».
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MetricId(pub u16);

impl MetricId {
    #[must_use]
    pub fn describe(self) -> Option<&'static MetricDesc> {
        describe(self)
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        describe(self).map_or("unknown", |d| d.name)
    }
}

impl fmt::Debug for MetricId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MetricId({}:{})", self.0, self.name())
    }
}

impl fmt::Display for MetricId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Тип метрики определяет и корректный downsampling, и семантику чтения.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MetricKind {
    /// Мгновенное значение. Сжатие: min/max/sum/count/last.
    Gauge,
    /// Монотонный счётчик. Хранится сырым, скорость считается на чтении.
    /// Сжатие: first/last/delta/resets.
    Counter,
}

/// Единица измерения. Нужна и для форматирования в TUI, и для суффикса в OpenMetrics.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Unit {
    /// Доля 0..1.
    Ratio,
    Bytes,
    /// Байт в секунду: скорость, а не накопленный объём.
    ///
    /// Отдельная единица нужна потому, что `/s` нельзя выводить из типа
    /// значения (раздел 154): накопительный счётчик байт и скорость передачи
    /// имеют одинаковую размерность и разный смысл. Раньше обе величины были
    /// помечены `Bytes`, и таблица показывала счётчик как «9.1 GiB/s».
    BytesPerSecond,
    Seconds,
    Microseconds,
    Milliseconds,
    Count,
    Cores,
    /// Безразмерная величина (load average, oom_score).
    None,
}

impl Unit {
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Unit::Ratio => "ratio",
            Unit::Bytes => "bytes",
            Unit::BytesPerSecond => "bytes_per_second",
            Unit::Seconds => "seconds",
            Unit::Microseconds => "microseconds",
            Unit::Milliseconds => "milliseconds",
            Unit::Count => "count",
            Unit::Cores => "cores",
            Unit::None => "",
        }
    }
}

/// Область применения метрики. Только для валидации и подсказок в UI.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MetricScope {
    Host,
    Disk,
    NetIf,
    Cgroup,
    Process,
    Agent,
}

/// Политика экспорта в `/metrics`.
///
/// Безопасность и cardinality: метрики уровня процесса по умолчанию не экспортируются,
/// потому что несут высокую cardinality и потенциально чувствительные имена.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ExportPolicy {
    /// Экспортируется всегда: низкая cardinality, безопасно.
    Always,
    /// Экспортируется только при явном включении в конфигурации.
    OptIn,
    /// Никогда не экспортируется (внутренняя производная величина).
    Never,
}

/// Статическое описание метрики.
#[derive(Copy, Clone, Debug)]
pub struct MetricDesc {
    pub id: MetricId,
    /// Имя без префикса `pulse_`.
    pub name: &'static str,
    pub kind: MetricKind,
    pub unit: Unit,
    pub scope: MetricScope,
    pub export: ExportPolicy,
    pub help: &'static str,
}

impl MetricDesc {
    /// Полное имя в OpenMetrics: `pulse_<name>[_total]`.
    #[must_use]
    pub fn prom_name(&self) -> String {
        match self.kind {
            MetricKind::Counter => format!("pulse_{}_total", self.name),
            MetricKind::Gauge => format!("pulse_{}", self.name),
        }
    }
}

macro_rules! metrics {
    ( $( $const_name:ident = $id:expr, $name:literal, $kind:ident, $unit:ident, $scope:ident, $export:ident, $help:literal ; )* ) => {
        /// Константы идентификаторов метрик.
        pub mod ids {
            use super::MetricId;
            $( pub const $const_name: MetricId = MetricId($id); )*
        }

        /// Таблица описаний, отсортированная по `id` (проверяется тестом).
        pub static DESCRIPTORS: &[MetricDesc] = &[
            $( MetricDesc {
                id: MetricId($id),
                name: $name,
                kind: MetricKind::$kind,
                unit: Unit::$unit,
                scope: MetricScope::$scope,
                export: ExportPolicy::$export,
                help: $help,
            }, )*
        ];
    };
}

metrics! {
    // ---- host: 1..=49 -----------------------------------------------------
    HOST_CPU_UTIL              = 1,  "host_cpu_utilization", Gauge, Ratio, Host, Always, "Доля занятого CPU по всем ядрам, 0..1";
    HOST_CPU_USER              = 2,  "host_cpu_user_ratio", Gauge, Ratio, Host, Always, "Доля времени CPU в user";
    HOST_CPU_SYSTEM            = 3,  "host_cpu_system_ratio", Gauge, Ratio, Host, Always, "Доля времени CPU в system";
    HOST_CPU_IOWAIT            = 4,  "host_cpu_iowait_ratio", Gauge, Ratio, Host, Always, "Доля времени CPU в iowait";
    HOST_CPU_STEAL             = 5,  "host_cpu_steal_ratio", Gauge, Ratio, Host, Always, "Доля украденного гипервизором времени CPU";
    HOST_CPU_SECONDS           = 6,  "host_cpu_seconds", Counter, Seconds, Host, Always, "Суммарное время CPU по всем режимам";
    HOST_CPU_COUNT             = 7,  "host_cpu_count", Gauge, Count, Host, Always, "Число логических CPU";
    HOST_LOAD1                 = 8,  "host_load1", Gauge, None, Host, Always, "Load average за 1 минуту";
    HOST_LOAD5                 = 9,  "host_load5", Gauge, None, Host, Always, "Load average за 5 минут";
    HOST_LOAD15                = 10, "host_load15", Gauge, None, Host, Always, "Load average за 15 минут";
    HOST_MEM_TOTAL             = 11, "host_memory_total_bytes", Gauge, Bytes, Host, Always, "Всего оперативной памяти";
    HOST_MEM_AVAILABLE         = 12, "host_memory_available_bytes", Gauge, Bytes, Host, Always, "Доступно памяти по оценке ядра";
    HOST_MEM_USED              = 13, "host_memory_used_bytes", Gauge, Bytes, Host, Always, "Занято памяти (total - available)";
    HOST_MEM_FREE              = 14, "host_memory_free_bytes", Gauge, Bytes, Host, Always, "Свободно памяти";
    HOST_MEM_CACHED            = 15, "host_memory_cached_bytes", Gauge, Bytes, Host, Always, "Память в страничном кеше";
    HOST_MEM_BUFFERS           = 16, "host_memory_buffers_bytes", Gauge, Bytes, Host, Always, "Память в буферах";
    HOST_MEM_UTIL              = 17, "host_memory_utilization", Gauge, Ratio, Host, Always, "Доля занятой памяти, 0..1";
    HOST_SWAP_TOTAL            = 18, "host_swap_total_bytes", Gauge, Bytes, Host, Always, "Всего swap";
    HOST_SWAP_USED             = 19, "host_swap_used_bytes", Gauge, Bytes, Host, Always, "Занято swap";
    HOST_PSI_CPU_SOME_AVG10    = 20, "host_pressure_cpu_some_avg10", Gauge, Ratio, Host, Always, "PSI cpu some avg10, доля 0..1";
    HOST_PSI_CPU_SOME_TOTAL    = 21, "host_pressure_cpu_some_seconds", Counter, Seconds, Host, Always, "PSI cpu some накопленное время задержки";
    HOST_PSI_MEM_SOME_AVG10    = 22, "host_pressure_memory_some_avg10", Gauge, Ratio, Host, Always, "PSI memory some avg10";
    HOST_PSI_MEM_FULL_AVG10    = 23, "host_pressure_memory_full_avg10", Gauge, Ratio, Host, Always, "PSI memory full avg10";
    HOST_PSI_MEM_FULL_TOTAL    = 24, "host_pressure_memory_full_seconds", Counter, Seconds, Host, Always, "PSI memory full накопленное время";
    HOST_PSI_IO_SOME_AVG10     = 25, "host_pressure_io_some_avg10", Gauge, Ratio, Host, Always, "PSI io some avg10";
    HOST_PSI_IO_FULL_AVG10     = 26, "host_pressure_io_full_avg10", Gauge, Ratio, Host, Always, "PSI io full avg10";
    HOST_PSI_IO_FULL_TOTAL     = 27, "host_pressure_io_full_seconds", Counter, Seconds, Host, Always, "PSI io full накопленное время";
    HOST_PROCS_RUNNING         = 28, "host_procs_running", Gauge, Count, Host, Always, "Число процессов в состоянии running";
    HOST_PROCS_BLOCKED         = 29, "host_procs_blocked", Gauge, Count, Host, Always, "Число процессов, заблокированных на IO";
    HOST_PROCS_TOTAL           = 30, "host_process_count", Gauge, Count, Host, Always, "Всего процессов";
    HOST_THREADS_TOTAL         = 31, "host_thread_count", Gauge, Count, Host, Always, "Всего потоков";
    HOST_CTX_SWITCHES          = 32, "host_context_switches", Counter, Count, Host, Always, "Переключения контекста";
    HOST_FORKS                 = 33, "host_forks", Counter, Count, Host, Always, "Созданные процессы";
    HOST_FD_OPEN               = 34, "host_file_descriptors_open", Gauge, Count, Host, Always, "Открытые файловые дескрипторы в системе";
    HOST_FD_MAX                = 35, "host_file_descriptors_max", Gauge, Count, Host, Always, "Лимит файловых дескрипторов";
    HOST_FD_UTIL               = 36, "host_file_descriptors_utilization", Gauge, Ratio, Host, Always, "Доля использованных дескрипторов";
    HOST_TCP_INUSE             = 37, "host_tcp_sockets_inuse", Gauge, Count, Host, Always, "TCP-сокеты в использовании";
    HOST_TCP_RETRANS           = 38, "host_tcp_retransmit_segments", Counter, Count, Host, Always, "Повторно переданные TCP-сегменты";
    HOST_UPTIME                = 39, "host_uptime_seconds", Gauge, Seconds, Host, Always, "Время работы системы";
    HOST_NET_RX_BYTES          = 40, "host_network_receive_bytes", Counter, Bytes, Host, Always, "Принято байт по всем интерфейсам";
    HOST_NET_TX_BYTES          = 41, "host_network_transmit_bytes", Counter, Bytes, Host, Always, "Передано байт по всем интерфейсам";
    HOST_DISK_READ_BYTES       = 42, "host_disk_read_bytes", Counter, Bytes, Host, Always, "Прочитано байт со всех дисков";
    HOST_DISK_WRITE_BYTES      = 43, "host_disk_write_bytes", Counter, Bytes, Host, Always, "Записано байт на все диски";
    HOST_OOM_KILLS             = 44, "host_oom_kills", Counter, Count, Host, Always, "Число событий OOM kill";
    HOST_NET_RX_THROUGHPUT     = 45, "host_network_receive_bytes_per_second", Gauge, BytesPerSecond, Host, Always, "Скорость приёма по всем интерфейсам";
    HOST_NET_TX_THROUGHPUT     = 46, "host_network_transmit_bytes_per_second", Gauge, BytesPerSecond, Host, Always, "Скорость передачи по всем интерфейсам";
    HOST_DISK_READ_THROUGHPUT  = 47, "host_disk_read_bytes_per_second", Gauge, BytesPerSecond, Host, Always, "Скорость чтения со всех дисков";
    HOST_DISK_WRITE_THROUGHPUT = 48, "host_disk_write_bytes_per_second", Gauge, BytesPerSecond, Host, Always, "Скорость записи на все диски";
    HOST_FS_ROOT_UTIL          = 49, "host_filesystem_root_utilization", Gauge, Ratio, Host, Always, "Заполненность корневой файловой системы";

    // ---- disk: 50..=69 ----------------------------------------------------
    DISK_READ_BYTES            = 50, "disk_read_bytes", Counter, Bytes, Disk, Always, "Прочитано байт с устройства";
    DISK_WRITE_BYTES           = 51, "disk_write_bytes", Counter, Bytes, Disk, Always, "Записано байт на устройство";
    DISK_READ_OPS              = 52, "disk_read_operations", Counter, Count, Disk, Always, "Завершённые операции чтения";
    DISK_WRITE_OPS             = 53, "disk_write_operations", Counter, Count, Disk, Always, "Завершённые операции записи";
    DISK_READ_WAIT             = 54, "disk_read_wait_milliseconds", Counter, Milliseconds, Disk, Always, "Суммарное время ожидания чтений";
    DISK_WRITE_WAIT            = 55, "disk_write_wait_milliseconds", Counter, Milliseconds, Disk, Always, "Суммарное время ожидания записей";
    DISK_IO_TIME               = 56, "disk_io_time_milliseconds", Counter, Milliseconds, Disk, Always, "Время, когда устройство было занято";
    DISK_INFLIGHT              = 57, "disk_inflight_operations", Gauge, Count, Disk, Always, "Операции в полёте";
    DISK_AWAIT                 = 58, "disk_await_milliseconds", Gauge, Milliseconds, Disk, Always, "Среднее время обслуживания запроса за такт";
    DISK_UTIL                  = 59, "disk_utilization", Gauge, Ratio, Disk, Always, "Доля времени, когда устройство занято";
    DISK_QUEUE                 = 60, "disk_queue_length", Gauge, None, Disk, Always, "Средняя длина очереди за такт";
    DISK_READ_THROUGHPUT       = 61, "disk_read_bytes_per_second", Gauge, BytesPerSecond, Disk, Always, "Скорость чтения за такт, байт/с";
    DISK_WRITE_THROUGHPUT      = 62, "disk_write_bytes_per_second", Gauge, BytesPerSecond, Disk, Always, "Скорость записи за такт, байт/с";
    // Файловые системы живут в хвосте блока дисков: у них та же природа
    // (носитель данных), но область действия - хост, а не устройство.
    HOST_FS_ROOT_TOTAL         = 66, "host_filesystem_root_total_bytes", Gauge, Bytes, Host, Always, "Размер корневой файловой системы";
    HOST_FS_ROOT_FREE          = 67, "host_filesystem_root_free_bytes", Gauge, Bytes, Host, Always, "Свободно в корневой файловой системе";
    HOST_FS_WORST_UTIL         = 68, "host_filesystem_worst_utilization", Gauge, Ratio, Host, Always, "Заполненность самой полной значимой файловой системы";

    // ---- network interface: 70..=89 ---------------------------------------
    NETIF_RX_BYTES             = 70, "netif_receive_bytes", Counter, Bytes, NetIf, Always, "Принято байт";
    NETIF_TX_BYTES             = 71, "netif_transmit_bytes", Counter, Bytes, NetIf, Always, "Передано байт";
    NETIF_RX_PACKETS           = 72, "netif_receive_packets", Counter, Count, NetIf, Always, "Принято пакетов";
    NETIF_TX_PACKETS           = 73, "netif_transmit_packets", Counter, Count, NetIf, Always, "Передано пакетов";
    NETIF_RX_ERRORS            = 74, "netif_receive_errors", Counter, Count, NetIf, Always, "Ошибки приёма";
    NETIF_TX_ERRORS            = 75, "netif_transmit_errors", Counter, Count, NetIf, Always, "Ошибки передачи";
    NETIF_RX_DROPS             = 76, "netif_receive_drops", Counter, Count, NetIf, Always, "Отброшено при приёме";
    NETIF_TX_DROPS             = 77, "netif_transmit_drops", Counter, Count, NetIf, Always, "Отброшено при передаче";
    NETIF_RX_THROUGHPUT        = 78, "netif_receive_bytes_per_second", Gauge, BytesPerSecond, NetIf, Always, "Скорость приёма за такт, байт/с";
    NETIF_TX_THROUGHPUT        = 79, "netif_transmit_bytes_per_second", Gauge, BytesPerSecond, NetIf, Always, "Скорость передачи за такт, байт/с";

    // ---- cgroup и владельцы: 90..=149 -------------------------------------
    CG_CPU_USAGE_USEC          = 90,  "cgroup_cpu_usage_microseconds", Counter, Microseconds, Cgroup, Always, "Использованное время CPU";
    CG_CPU_USER_USEC           = 91,  "cgroup_cpu_user_microseconds", Counter, Microseconds, Cgroup, Always, "Время CPU в user";
    CG_CPU_SYSTEM_USEC         = 92,  "cgroup_cpu_system_microseconds", Counter, Microseconds, Cgroup, Always, "Время CPU в system";
    CG_CPU_CORES               = 93,  "cgroup_cpu_cores", Gauge, Cores, Cgroup, Always, "Потребление CPU в ядрах за такт";
    CG_CPU_LIMIT_CORES         = 94,  "cgroup_cpu_limit_cores", Gauge, Cores, Cgroup, Always, "Лимит CPU в ядрах (cpu.max), 0 если не задан";
    CG_CPU_NR_PERIODS          = 95,  "cgroup_cpu_periods", Counter, Count, Cgroup, Always, "Число периодов throttling";
    CG_CPU_NR_THROTTLED        = 96,  "cgroup_cpu_throttled_periods", Counter, Count, Cgroup, Always, "Число периодов с throttling";
    CG_CPU_THROTTLED_USEC      = 97,  "cgroup_cpu_throttled_microseconds", Counter, Microseconds, Cgroup, Always, "Время в throttling";
    CG_CPU_THROTTLE_RATIO      = 98,  "cgroup_cpu_throttle_ratio", Gauge, Ratio, Cgroup, Always, "Доля периодов с throttling за такт";
    CG_MEM_CURRENT             = 99,  "cgroup_memory_current_bytes", Gauge, Bytes, Cgroup, Always, "Текущее потребление памяти";
    CG_MEM_LIMIT               = 100, "cgroup_memory_limit_bytes", Gauge, Bytes, Cgroup, Always, "Лимит памяти (memory.max), 0 если не задан";
    CG_MEM_UTIL                = 101, "cgroup_memory_utilization", Gauge, Ratio, Cgroup, Always, "Доля лимита памяти";
    CG_MEM_ANON                = 102, "cgroup_memory_anon_bytes", Gauge, Bytes, Cgroup, Always, "Анонимная память";
    CG_MEM_FILE                = 103, "cgroup_memory_file_bytes", Gauge, Bytes, Cgroup, Always, "Файловая память";
    CG_MEM_SWAP_CURRENT        = 104, "cgroup_memory_swap_bytes", Gauge, Bytes, Cgroup, Always, "Память в swap";
    CG_MEM_EVENTS_HIGH         = 105, "cgroup_memory_events_high", Counter, Count, Cgroup, Always, "Срабатывания memory.high";
    CG_MEM_EVENTS_MAX          = 106, "cgroup_memory_events_max", Counter, Count, Cgroup, Always, "Срабатывания memory.max";
    CG_MEM_EVENTS_OOM          = 107, "cgroup_memory_events_oom", Counter, Count, Cgroup, Always, "События OOM";
    CG_MEM_EVENTS_OOM_KILL     = 108, "cgroup_memory_events_oom_kill", Counter, Count, Cgroup, Always, "События OOM kill";
    CG_IO_READ_BYTES           = 109, "cgroup_io_read_bytes", Counter, Bytes, Cgroup, Always, "Прочитано байт";
    CG_IO_WRITE_BYTES          = 110, "cgroup_io_write_bytes", Counter, Bytes, Cgroup, Always, "Записано байт";
    CG_IO_READ_OPS             = 111, "cgroup_io_read_operations", Counter, Count, Cgroup, Always, "Операции чтения";
    CG_IO_WRITE_OPS            = 112, "cgroup_io_write_operations", Counter, Count, Cgroup, Always, "Операции записи";
    CG_PSI_CPU_SOME_AVG10      = 113, "cgroup_pressure_cpu_some_avg10", Gauge, Ratio, Cgroup, Always, "PSI cpu some avg10 для cgroup";
    CG_PSI_MEM_FULL_AVG10      = 114, "cgroup_pressure_memory_full_avg10", Gauge, Ratio, Cgroup, Always, "PSI memory full avg10 для cgroup";
    CG_PSI_IO_FULL_AVG10       = 115, "cgroup_pressure_io_full_avg10", Gauge, Ratio, Cgroup, Always, "PSI io full avg10 для cgroup";
    CG_PSI_CPU_SOME_TOTAL      = 116, "cgroup_pressure_cpu_some_seconds", Counter, Seconds, Cgroup, Always, "PSI cpu some накопленное время";
    CG_PSI_MEM_FULL_TOTAL      = 117, "cgroup_pressure_memory_full_seconds", Counter, Seconds, Cgroup, Always, "PSI memory full накопленное время";
    CG_PSI_IO_FULL_TOTAL       = 118, "cgroup_pressure_io_full_seconds", Counter, Seconds, Cgroup, Always, "PSI io full накопленное время";
    CG_PIDS_CURRENT            = 119, "cgroup_pids_current", Gauge, Count, Cgroup, Always, "Число задач в cgroup";
    CG_PIDS_MAX                = 120, "cgroup_pids_limit", Gauge, Count, Cgroup, Always, "Лимит числа задач, 0 если не задан";
    CG_PIDS_UTIL               = 121, "cgroup_pids_utilization", Gauge, Ratio, Cgroup, Always, "Доля лимита задач";
    CG_PROCESS_COUNT           = 122, "cgroup_process_count", Gauge, Count, Cgroup, Always, "Число процессов, отнесённых к cgroup";
    CG_IO_READ_THROUGHPUT      = 123, "cgroup_io_read_bytes_per_second", Gauge, BytesPerSecond, Cgroup, Always, "Скорость чтения за такт";
    CG_IO_WRITE_THROUGHPUT     = 124, "cgroup_io_write_bytes_per_second", Gauge, BytesPerSecond, Cgroup, Always, "Скорость записи за такт";

    // ---- process: 150..=189 ----------------------------------------------
    PROC_CPU_CORES             = 150, "process_cpu_cores", Gauge, Cores, Process, OptIn, "Потребление CPU процессом в ядрах за такт";
    PROC_CPU_SECONDS           = 151, "process_cpu_seconds", Counter, Seconds, Process, OptIn, "Накопленное время CPU процесса";
    PROC_CPU_USER_SECONDS      = 152, "process_cpu_user_seconds", Counter, Seconds, Process, OptIn, "Время CPU процесса в user";
    PROC_CPU_SYSTEM_SECONDS    = 153, "process_cpu_system_seconds", Counter, Seconds, Process, OptIn, "Время CPU процесса в system";
    PROC_RSS                   = 154, "process_resident_memory_bytes", Gauge, Bytes, Process, OptIn, "Resident set size";
    PROC_VMS                   = 155, "process_virtual_memory_bytes", Gauge, Bytes, Process, OptIn, "Virtual memory size";
    PROC_SHARED                = 156, "process_shared_memory_bytes", Gauge, Bytes, Process, OptIn, "Разделяемые страницы";
    PROC_MINOR_FAULTS          = 157, "process_minor_page_faults", Counter, Count, Process, OptIn, "Minor page faults";
    PROC_MAJOR_FAULTS          = 158, "process_major_page_faults", Counter, Count, Process, OptIn, "Major page faults";
    PROC_IO_READ_BYTES         = 159, "process_io_read_bytes", Counter, Bytes, Process, OptIn, "Прочитано байт процессом";
    PROC_IO_WRITE_BYTES        = 160, "process_io_write_bytes", Counter, Bytes, Process, OptIn, "Записано байт процессом";
    PROC_IO_READ_SYSCALLS      = 161, "process_io_read_syscalls", Counter, Count, Process, OptIn, "Системные вызовы чтения";
    PROC_IO_WRITE_SYSCALLS     = 162, "process_io_write_syscalls", Counter, Count, Process, OptIn, "Системные вызовы записи";
    PROC_FD_COUNT              = 163, "process_open_file_descriptors", Gauge, Count, Process, OptIn, "Открытые дескрипторы процесса";
    PROC_FD_LIMIT              = 164, "process_file_descriptor_limit", Gauge, Count, Process, OptIn, "Лимит дескрипторов процесса";
    PROC_FD_UTIL               = 165, "process_file_descriptor_utilization", Gauge, Ratio, Process, OptIn, "Доля использованных дескрипторов";
    PROC_THREADS               = 166, "process_threads", Gauge, Count, Process, OptIn, "Число потоков процесса";
    PROC_OOM_SCORE             = 167, "process_oom_score", Gauge, None, Process, OptIn, "oom_score процесса";
    PROC_NICE                  = 168, "process_nice", Gauge, None, Process, OptIn, "nice-приоритет";
    PROC_UPTIME                = 169, "process_uptime_seconds", Gauge, Seconds, Process, OptIn, "Время жизни процесса";
    PROC_CTX_VOLUNTARY         = 170, "process_voluntary_context_switches", Counter, Count, Process, OptIn, "Добровольные переключения контекста";
    PROC_CTX_INVOLUNTARY       = 171, "process_involuntary_context_switches", Counter, Count, Process, OptIn, "Принудительные переключения контекста";
    PROC_STATE_CODE            = 172, "process_state_code", Gauge, None, Process, Never, "Состояние процесса как код: 0 R, 1 S, 2 D, 3 T, 4 Z, 5 иное";

    // ---- самонаблюдение агента: 190..=219 --------------------------------
    AGENT_TICK_DURATION        = 190, "agent_tick_duration_milliseconds", Gauge, Milliseconds, Agent, Always, "Длительность такта сбора";
    AGENT_TICK_SKIPPED         = 191, "agent_ticks_skipped", Counter, Count, Agent, Always, "Пропущенные такты из-за перегрузки";
    AGENT_TICKS                = 192, "agent_ticks", Counter, Count, Agent, Always, "Выполненные такты сбора";
    AGENT_ENTITIES_LIVE        = 193, "agent_entities_live", Gauge, Count, Agent, Always, "Живые сущности в графе";
    AGENT_SERIES_LIVE          = 194, "agent_series_live", Gauge, Count, Agent, Always, "Активные серии в хранилище";
    AGENT_SAMPLES_STORED       = 195, "agent_samples_stored", Counter, Count, Agent, Always, "Записанные образцы";
    AGENT_STORE_BYTES          = 196, "agent_store_bytes", Gauge, Bytes, Agent, Always, "Оценка памяти хранилища";
    AGENT_EVENTS_TOTAL         = 197, "agent_events", Counter, Count, Agent, Always, "Записанные события";
    AGENT_PROBLEMS_OPEN        = 198, "agent_problems_open", Gauge, Count, Agent, Always, "Открытые проблемы";
    AGENT_COLLECTOR_ERRORS     = 199, "agent_collector_errors", Counter, Count, Agent, Always, "Ошибки коллекторов";
    AGENT_RSS                  = 200, "agent_resident_memory_bytes", Gauge, Bytes, Agent, Always, "RSS самого агента";
    AGENT_CPU_SECONDS          = 201, "agent_cpu_seconds", Counter, Seconds, Agent, Always, "Время CPU самого агента";
    AGENT_EXPORT_SERIES        = 202, "agent_export_series", Gauge, Count, Agent, Always, "Серии, отданные в последнем scrape";
    AGENT_EXPORT_DROPPED       = 203, "agent_export_series_dropped", Counter, Count, Agent, Always, "Серии, отброшенные бюджетом cardinality";
    AGENT_EXPORT_REQUESTS      = 204, "agent_export_requests", Counter, Count, Agent, Always, "Обработанные HTTP-запросы экспорта";
    AGENT_EXPORT_REJECTED      = 205, "agent_export_requests_rejected", Counter, Count, Agent, Always, "Отклонённые HTTP-запросы экспорта";
    AGENT_REDACTIONS           = 206, "agent_redactions", Counter, Count, Agent, Always, "Число скрытых потенциальных секретов";
}

/// Метрики, которым нужен длинный ретеншн — тёплый слой истории.
///
/// Тёплый слой существует ради A/B diff и статистики по длинному окну, а не ради
/// «пусть всё хранится». Хранить в нём все серии владельцев — значит платить
/// памятью за данные, которые никто не запрашивает: на типовом хосте это тысячи
/// серий против сотен нужных. Список — единственный источник правды: `pulse-store`
/// решает по нему, что кладётся в тёплый слой, а `pulse-engine` берёт его же как
/// набор сравниваемых метрик (см. тест `watched_metrics_are_retained_long`).
///
/// Метрика вне списка живёт только в горячем кольце: запрос за его пределами
/// вернёт `None`, а не приблизительную оценку. Это честнее, чем тихо отдавать
/// агрегат там, где точности уже нет.
///
/// Таблица обязана быть отсортированной и уникальной (проверяется тестом).
pub static LONG_WINDOW: &[MetricId] = &[
    ids::HOST_CPU_UTIL,
    ids::HOST_LOAD1,
    ids::HOST_MEM_UTIL,
    ids::HOST_PSI_CPU_SOME_AVG10,
    ids::HOST_PSI_MEM_FULL_AVG10,
    ids::HOST_PSI_IO_FULL_AVG10,
    ids::HOST_FD_UTIL,
    ids::DISK_AWAIT,
    ids::DISK_UTIL,
    ids::DISK_QUEUE,
    ids::NETIF_RX_THROUGHPUT,
    ids::NETIF_TX_THROUGHPUT,
    ids::CG_CPU_CORES,
    ids::CG_CPU_THROTTLE_RATIO,
    ids::CG_MEM_CURRENT,
    ids::CG_MEM_UTIL,
    ids::CG_PSI_CPU_SOME_AVG10,
    ids::CG_PSI_MEM_FULL_AVG10,
    ids::CG_PSI_IO_FULL_AVG10,
    ids::CG_PIDS_CURRENT,
    ids::PROC_CPU_CORES,
    ids::PROC_RSS,
];

/// Нужен ли метрике длинный ретеншн. Бинарный поиск по [`LONG_WINDOW`].
#[must_use]
pub fn retains_long_window(id: MetricId) -> bool {
    LONG_WINDOW.binary_search(&id).is_ok()
}

/// Описание метрики по идентификатору. Бинарный поиск по отсортированной таблице.
#[must_use]
pub fn describe(id: MetricId) -> Option<&'static MetricDesc> {
    DESCRIPTORS
        .binary_search_by_key(&id.0, |d| d.id.0)
        .ok()
        .and_then(|idx| DESCRIPTORS.get(idx))
}

/// Все метрики указанной области.
pub fn scope_metrics(scope: MetricScope) -> impl Iterator<Item = &'static MetricDesc> {
    DESCRIPTORS.iter().filter(move |d| d.scope == scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn long_window_list_is_sorted_unique_and_declared() {
        let mut prev: Option<MetricId> = None;
        for id in LONG_WINDOW {
            if let Some(previous) = prev {
                assert!(
                    id.0 > previous.0,
                    "список длинного ретеншна должен строго возрастать: {} после {}",
                    id.0,
                    previous.0
                );
            }
            assert!(
                describe(*id).is_some(),
                "метрика {} отсутствует в реестре",
                id.0
            );
            prev = Some(*id);
        }
        assert!(retains_long_window(ids::CG_MEM_CURRENT));
        assert!(!retains_long_window(ids::CG_MEM_ANON));
    }

    #[test]
    fn long_window_is_a_small_share_of_the_registry() {
        // Инвариант стоимости: тёплый слой обслуживает единицы процентов серий.
        // Если список начнёт разрастаться, память вернётся к прежнему уровню.
        assert!(
            LONG_WINDOW.len() * 4 < DESCRIPTORS.len(),
            "длинный ретеншн у {} из {} метрик — слишком много",
            LONG_WINDOW.len(),
            DESCRIPTORS.len()
        );
    }
    #[test]
    fn descriptor_table_is_sorted_and_unique() {
        let mut prev = 0u16;
        for d in DESCRIPTORS {
            assert!(
                d.id.0 > prev,
                "таблица метрик должна быть строго возрастающей: {} после {}",
                d.id.0,
                prev
            );
            prev = d.id.0;
        }
    }

    #[test]
    fn metric_names_are_unique() {
        let mut names = HashSet::new();
        for d in DESCRIPTORS {
            assert!(
                names.insert(d.name),
                "дублирующееся имя метрики: {}",
                d.name
            );
        }
    }

    #[test]
    fn prometheus_names_are_unique_and_valid() {
        let mut names = HashSet::new();
        for d in DESCRIPTORS {
            let name = d.prom_name();
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "недопустимое имя для OpenMetrics: {name}"
            );
            assert!(names.insert(name.clone()), "дублирующееся имя: {name}");
        }
    }

    #[test]
    fn describe_finds_every_declared_metric() {
        for d in DESCRIPTORS {
            let found = describe(d.id).expect("метрика должна находиться");
            assert_eq!(found.name, d.name);
        }
        assert!(describe(MetricId(0)).is_none());
        assert!(describe(MetricId(u16::MAX)).is_none());
    }

    #[test]
    fn process_metrics_are_not_exported_by_default() {
        for d in scope_metrics(MetricScope::Process) {
            assert_ne!(
                d.export,
                ExportPolicy::Always,
                "метрика процесса {} не должна экспортироваться по умолчанию",
                d.name
            );
        }
    }

    #[test]
    fn counters_carry_total_suffix() {
        for d in DESCRIPTORS {
            let name = d.prom_name();
            match d.kind {
                MetricKind::Counter => assert!(name.ends_with("_total")),
                MetricKind::Gauge => assert!(!name.ends_with("_total")),
            }
        }
    }
}
