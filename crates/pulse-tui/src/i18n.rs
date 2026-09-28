//! Две локали пользовательского интерфейса.
//!
//! Внутренние идентификаторы, имена метрик и данные ядра не переводятся:
//! язык меняет только подписи и пояснения оператора. Английская строка служит
//! стабильным ключом, поэтому экран не заводит свой второй словарь терминов.

use std::borrow::Cow;

use pulse_core::config::{IconSet, Language};

#[must_use]
pub const fn choose(
    language: Language,
    english: &'static str,
    russian: &'static str,
) -> &'static str {
    match language {
        Language::English => english,
        Language::Russian => russian,
    }
}

/// Переводит стабильную подпись интерфейса. Неизвестные значения — имена
/// сущностей и данные ядра — возвращаются дословно.
#[must_use]
pub fn translate(language: Language, text: &str) -> Cow<'_, str> {
    if language == Language::English {
        return Cow::Borrowed(text);
    }
    let translated = match text {
        "OVERVIEW" => "ОБЗОР",
        "PROBLEMS" => "ПРОБЛЕМЫ",
        "ENTITIES" => "ОБЪЕКТЫ",
        "TIME MACHINE" => "ИСТОРИЯ",
        "INSPECTOR" => "ИНСПЕКТОР",
        "COMMANDS" => "КОМАНДЫ",
        "HELP" => "СПРАВКА",
        "SEARCH" => "ПОИСК",
        "PIPE" => "РАССЛЕДОВАНИЕ",
        "SETTINGS" => "НАСТРОЙКИ",
        "STATE" => "СОСТОЯНИЕ",
        "ATTENTION" => "ВНИМАНИЕ",
        "SIGNALS" => "СИГНАЛЫ",
        "RECENT CHANGES" => "ПОСЛЕДНИЕ ИЗМЕНЕНИЯ",
        "RELEVANT ENTITIES" => "ЗНАЧИМЫЕ ОБЪЕКТЫ",
        "KEY ENTITIES" => "КЛЮЧЕВЫЕ ОБЪЕКТЫ",
        "SELECTED" => "ВЫБРАНО",
        "RELEVANCE" => "ЗНАЧИМОСТЬ",
        "ENTITY LIST" => "СПИСОК ОБЪЕКТОВ",
        "PREVIEW" => "ПРОСМОТР",
        "SUMMARY" => "СВОДКА",
        "RELATIONS" => "СВЯЗИ",
        "LABELS" => "МЕТКИ",
        "CURRENT PROBLEMS" => "ТЕКУЩИЕ ПРОБЛЕМЫ",
        "RECENT RESOLVED" => "НЕДАВНО РЕШЕНЫ",
        "WHY" => "ПОЧЕМУ",
        "AFFECTED" => "ЗАТРОНУТО",
        "RECENT" => "НЕДАВНО",
        "STORY" => "ИСТОРИЯ",
        "RAW EVENTS" => "СЫРЫЕ СОБЫТИЯ",
        "SNAPSHOT / DETAILS" => "СНИМОК / ДЕТАЛИ",
        "BASELINE" => "БАЗОВАЯ ТОЧКА",
        "EVENTS" => "СОБЫТИЯ",
        "TRAIL" => "СЛЕД",
        "CASE" => "ДЕЛО",
        "LEADS" => "ЗАЦЕПКИ",
        "INSIDE" => "ВНУТРИ",
        "AROUND" => "ВОКРУГ",
        "DOSSIER" => "ДОСЬЕ",
        "WHY IT EXISTS (HEURISTIC)" => "ПОЧЕМУ СУЩЕСТВУЕТ (ЭВРИСТИКА)",
        "NO ACTIVE PROBLEMS" => "НЕТ АКТИВНЫХ ПРОБЛЕМ",
        "NO DATA" => "НЕТ ДАННЫХ",
        "FUNCTION LOST" => "ФУНКЦИЯ ПОТЕРЯНА",
        "NAME" => "ИМЯ",
        "MEM" => "ПАМ",
        "KIND" => "ВИД",
        "OWNER" => "ВЛАДЕЛЕЦ",
        "TREND" => "ТРЕНД",
        "Language" => "Язык",
        "Icons" => "Иконки",
        "Preview" => "Предпросмотр",
        "English" => "English",
        "Russian" => "Русский",
        "Off" => "Выкл",
        "Unicode" => "Unicode",
        "Nerd Font" => "Nerd Font",
        "LIVE" => "ЭФИР",
        "PAUSED" => "ПАУЗА",
        "HISTORY" => "ИСТОРИЯ",
        "HIST" => "ИСТ",
        "NOW" => "СЕЙЧАС",
        "DEMO" => "ДЕМО",
        // Палитра команд и справка.
        "GLOBAL" => "ОБЩИЕ",
        "TIMELINE" => "ИСТОРИЯ",
        "Open selected" => "Открыть выбранное",
        "Next list" => "Следующий список",
        "Previous list" => "Предыдущий список",
        "Back along the trail" => "Назад по следу",
        "Next pane" => "Следующая панель",
        "Inspect problem entity" => "Открыть объект проблемы",
        "Next sort" => "Следующая сортировка",
        "Next type filter" => "Следующий фильтр типа",
        "Logical / technical view" => "Логический / технический вид",
        "Open selected event" => "Открыть выбранное событие",
        "Scrub back" => "Назад по времени",
        "Scrub forward" => "Вперёд по времени",
        "Back to LIVE" => "Вернуться в эфир",
        "Zoom in" => "Приблизить",
        "Zoom out" => "Отдалить",
        "Set mark A" => "Поставить метку A",
        "Set mark B" => "Поставить метку B",
        "Semantic A/B diff" => "Смысловая разница A/B",
        "Raw events" => "Сырые события",
        "Incident story" => "История инцидента",
        "Overview" => "Обзор",
        "Problems" => "Проблемы",
        "Entities" => "Объекты",
        "Timeline / Time Machine" => "История / машина времени",
        "Search" => "Поиск",
        "Pipe" => "Расследование",
        "Pause display" => "Пауза экрана",
        "Settings" => "Настройки",
        "Help" => "Справка",
        "Quit" => "Выход",
        "follow the selected row" => "перейти по выбранной строке",
        "inside → leads → around" => "внутри → зацепки → вокруг",
        "around → leads → inside" => "вокруг → зацепки → внутри",
        "one step back; closes at the start" => "шаг назад; в начале закрывает",
        "inspect the selected entity" => "исследовать выбранный объект",
        "entities ↔ selected preview" => "объекты ↔ просмотр выбранного",
        "open the entity of the selected problem" => "открыть объект выбранной проблемы",
        "memory, cpu, name, relevance" => "память, CPU, имя, значимость",
        "all, process, container, unit, cgroup" => "все, процесс, контейнер, unit, cgroup",
        "fold processes into services or not" => "сворачивать процессы в сервисы или нет",
        "list ↔ preview" => "список ↔ просмотр",
        "inspect the entity of the event" => "исследовать объект события",
        "one second into the past" => "на секунду в прошлое",
        "one second towards now" => "на секунду к настоящему",
        "follow the present again" => "снова следовать за настоящим",
        "narrow the time window" => "сузить временное окно",
        "widen the time window" => "расширить временное окно",
        "comparison start" => "начало сравнения",
        "comparison end" => "конец сравнения",
        "what changed between the marks" => "что изменилось между метками",
        "secondary stream of every observation" => "вторичный поток всех наблюдений",
        "back from raw events" => "назад из сырых событий",
        "rail, story, snapshot" => "ось, история, снимок",
        "system state and relevant entities" => "состояние системы и значимые объекты",
        "open problems with evidence" => "открытые проблемы с доказательствами",
        "full inventory" => "полный инвентарь",
        "story, lanes, A/B compare" => "история, дорожки, сравнение A/B",
        "filter entities by name or command" => "фильтр объектов по имени или команде",
        "facts about the selected entity" => "факты о выбранном объекте",
        "collection continues" => "сбор продолжается",
        "language and icon set" => "язык и набор иконок",
        "every command of every screen" => "все команды всех экранов",
        "leave PULSE" => "выйти из PULSE",
        // Алфавит состояний.
        "background: no contribution" => "фон: нет вклада",
        "normal edge / reference" => "нормальная граница / ориентир",
        "normal active mass" => "нормальная активная масса",
        "saturated but controlled" => "насыщено, но под контролем",
        "degraded" => "деградация",
        "warning" => "предупреждение",
        "critical" => "критично",
        "failed" => "отказ",
        "relevance" => "значимость",
        "technical" => "технический",
        "logical" => "логический",
        "all" => "все",
        "owner" => "владелец",
        "parent" => "родитель",
        "runs in" => "запущен в",
        "member of" => "в составе",
        "backed by" => "хранится на",
        "connects" => "соединён с",
        "processes" => "процессы",
        "kind" => "тип",
        "cmd" => "команда",
        "CPU 60s" => "CPU 60с",
        "children" => "потомки",
        "resources" => "ресурсы",
        "user" => "пользователь",
        "uid" => "uid",
        "exe" => "файл",
        "cwd" => "каталог",
        "journal" => "журнал",
        "listens" => "слушает",
        "files" => "файлы",
        "process" => "процесс",
        "source" => "источник",
        "coverage" => "охват",
        "ancestry" => "предки",
        "ports" => "порты",
        "limits" => "лимиты",
        "log" => "лог",
        "procs" => "процессы",
        "problems" => "проблемы",
        "problem" => "проблема",
        "changed" => "изменён",
        "cpu" => "cpu",
        "memory" => "память",
        "io" => "io",
        "network" => "сеть",
        "system" => "система",
        "pinned" => "закреплён",
        "selected" => "выбран",
        "observer" => "наблюдатель",
        "key" => "ключ",
        "entities" => "объекты",
        "cgroups" => "cgroup",
        "units" => "юниты",
        "none" => "нет",
        "not measured" => "не измерено",
        "unlimited" => "без лимита",
        "PULSE observation started" => "наблюдение PULSE началось",
        _ => {
            if let Some(count) = text
                .strip_prefix("baseline: ")
                .and_then(|rest| rest.strip_suffix(" entities"))
            {
                return Cow::Owned(format!("база: {count} объектов"));
            }
            for (prefix, replacement) in [
                ("SELECTED / ", "ВЫБРАНО / "),
                ("PREVIEW / ", "ПРОСМОТР / "),
                ("SNAPSHOT / ", "СНИМОК / "),
                ("INSIDE (", "ВНУТРИ ("),
            ] {
                if let Some(rest) = text.strip_prefix(prefix) {
                    return Cow::Owned(format!("{replacement}{rest}"));
                }
            }
            return Cow::Borrowed(text);
        }
    };
    Cow::Borrowed(translated)
}

#[must_use]
pub const fn language_name(ui: Language, value: Language) -> &'static str {
    match (ui, value) {
        (Language::English, Language::English) => "English",
        (Language::English, Language::Russian) => "Russian",
        (Language::Russian, Language::English) => "English",
        (Language::Russian, Language::Russian) => "Русский",
    }
}

#[must_use]
pub const fn icon_name(ui: Language, value: IconSet) -> &'static str {
    match (ui, value) {
        (Language::English, IconSet::Off) => "Off",
        (Language::Russian, IconSet::Off) => "Выкл",
        (_, IconSet::Unicode) => "Unicode",
        (_, IconSet::Nerd) => "Nerd Font",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_titles_keep_the_entity_name() {
        assert_eq!(
            translate(Language::Russian, "SELECTED / nginx.service"),
            "ВЫБРАНО / nginx.service"
        );
    }

    #[test]
    fn unknown_kernel_data_is_never_translated() {
        assert_eq!(
            translate(Language::Russian, "docker-a1b2.scope"),
            "docker-a1b2.scope"
        );
    }
}
