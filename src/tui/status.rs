//! Builds the terminal status card from the latest status and watcher snapshots.
//!
//! The rules that shaped it, each one a bug that shipped before:
//!
//! * **"Работает" needs proof.** Only a model answer in the client's own log
//!   turns the card green. Silence after a refusal used to read as "fixed" -
//!   and silence is also exactly what a user who gave up produces (G50).
//! * **One action at most, and a real one.** A card that says what is wrong
//!   carries the button that fixes it, or says plainly that there is nothing
//!   to press.
//! * DNS service health and client model answers are separate observations.

use std::time::Duration;

use crate::gate::View;
use crate::ops::{Cap, Status};

pub const VPN_TRAFFIC_NOTICE: &str = "Трафик анлокера идёт через VPN.\n\
    Если возникают ошибки, добавьте анлокер в исключения приложения VPN.";

/// How the card is coloured, and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Proven working.
    Ok,
    /// Set up, nothing proven yet: waiting for the user's next message.
    Wait,
    /// Something only the user can do.
    Action,
    /// Switched off, or nothing to work on.
    Off,
}

/// The one button a card may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Turn everything on.
    EnableAll,
    /// Restart this program elevated.
    Elevate,
    /// Reinstall and restart the service.
    Repair,
    /// Open Google's verification page (`Facts::verify`) in the browser.
    Verify,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::EnableAll => "Включить всё",
            Action::Elevate => "Перезапустить от имени администратора",
            Action::Repair => "Починить",
            Action::Verify => "Пройти верификацию в браузере",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub tone: Tone,
    pub title: String,
    pub detail: String,
    pub action: Option<Action>,
}

/// Everything the verdict is made of. Plain values, so a test can build any
/// state the window can be in.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Windows and elevated (or not Windows, where nothing needs it).
    pub admin: bool,
    /// At least one Antigravity install was found.
    pub installs_found: bool,
    /// The account patch is on everywhere it can be.
    pub patch_on: bool,
    /// The Windows DNS integration is on.
    pub bypass_on: bool,
    /// DNS integration is available on this platform.
    pub bypass_available: bool,
    pub relay_running: bool,
    pub relay_uses_vpn: bool,
    pub relay_outdated: bool,
    /// Our DNS rules are in. Without them nothing on the machine asks the
    /// service anything, however healthy it is.
    pub rules: bool,
    /// The relay's record is fresh, i.e. it is alive and saying what it does.
    pub relay_reporting: bool,
    /// The newest refusal in a client log (over the same twelve hours as
    /// `answer`): how long ago, and how many lines in the last ten minutes.
    pub refusal: Option<(Duration, usize)>,
    /// The newest model answer in a client log, how long ago.
    pub answer: Option<Duration>,
    /// What keeps the DNS relay off `127.0.0.53:53`. This listener cannot be
    /// moved - the NRPT rules name an address, not a port -
    /// so naming the holder is the whole of what can be done about it.
    pub dns_blocked: Option<crate::gate::Blocker>,
    /// The relay has run for minutes and nothing on the internet answered it.
    pub cut_off: bool,
    /// Whether this window itself reaches the internet - asked only while the
    /// relay is cut off, and what tells "no internet" from "something blocks
    /// the relay alone".
    pub net_ok: Option<bool>,
    /// The relay's own exe: the file an antivirus exception has to name.
    pub relay_exe: String,
    /// The newest time Google asked for the account to be verified: how long
    /// ago, and the page it gave. `headline` decides whether it still stands.
    pub verify: Option<(Duration, String)>,
    /// The newest 503 "no capacity" in the last ten minutes: how long ago, and
    /// the model it named (empty if none).
    pub overloaded: Option<(Duration, String)>,
    /// The newest 401 in the last ten minutes: how long ago, and how many.
    pub unauthorized: Option<(Duration, usize)>,
    /// macOS only: an install is unpatched and its `.app` bundle refuses the
    /// write (App Management). The card then says what to grant instead of
    /// offering a patch button that fails the same way. Always false elsewhere.
    pub patch_blocked: bool,
}

/// One 401 can be a token refreshed a moment late; a credential Google stopped
/// accepting fails every call, hundreds of times (G11).
const UNAUTH_LINES: usize = 2;

/// How long a verification demand stays on the card with no answer after it.
/// The page carries a one-time token; past this the client has usually asked
/// again and logged a fresh one, and an old link is more likely to fail than
/// help.
const VERIFY_FOR: Duration = Duration::from_secs(30 * 60);

/// About three refused turns (each writes four lines) with no answer in
/// between: past that, "send it again" has been tried and did not help.
const STUCK_LINES: usize = 12;

/// How long a refusal is current enough to request immediate action.
const REFUSAL_RECENT: Duration = Duration::from_secs(10 * 60);

impl Facts {
    /// Read off a status snapshot and the gate watcher's last view, which is
    /// `gate_age` old: the watcher sends an age measured at its own tick and
    /// then stays quiet while nothing changes, so the front end ages its copy.
    pub fn read(s: &Status, gate: &View, gate_age: Duration) -> Facts {
        let aged = |ago: Duration| ago + gate_age;
        // Newest over twelve hours, so an old refusal still outranks an older
        // answer; counted over the last ten minutes, which is what "it keeps
        // happening" means.
        let refusal = gate
            .refused_long
            .map(|x| (aged(x.ago), gate.seen.map_or(0, |s| s.count)));
        let answer = gate.answered.map(|x| aged(x.ago));
        // The relay's record is only worth anything while the relay runs: a record
        // outlives its writer by up to `STALE_AFTER`, and "обход перехватил" about a
        // dead service is the worst sentence this card could say (I58).
        let relay = gate.relay.as_ref().filter(|_| s.relay_running);
        Facts {
            admin: s.admin || !cfg!(target_os = "windows"),
            installs_found: s.installs.iter().any(|r| r.path.is_some()),
            patch_on: s.client_patch.is_on(),
            bypass_on: s.bypass_on(),
            bypass_available: cfg!(target_os = "windows"),
            relay_running: s.relay_running,
            relay_uses_vpn: s.relay_running && relay.is_some_and(|r| r.vpn_traffic),
            relay_outdated: s.relay_outdated,
            // Rules are only owed while the DNS switch is on.
            rules: s.rules || !s.dns.is_on() || !cfg!(target_os = "windows"),
            relay_reporting: relay.is_some(),
            refusal,
            answer,
            dns_blocked: blocker(relay, "dns"),
            cut_off: relay.is_some_and(crate::gate::Report::cut_off),
            net_ok: gate.net_ok,
            relay_exe: relay.map(|r| r.exe.clone()).unwrap_or_default(),
            verify: gate.verify.as_ref().map(|v| (aged(v.ago), v.url.clone())),
            overloaded: gate
                .overloaded
                .as_ref()
                .map(|o| (aged(o.ago), o.model.clone())),
            unauthorized: gate.unauthorized.map(|u| (aged(u.ago), u.count)),
            patch_blocked: s.patch_blocked,
        }
    }
}

fn blocker(relay: Option<&crate::gate::Report>, what: &str) -> Option<crate::gate::Blocker> {
    relay?.blockers.iter().find(|b| b.what == what).cloned()
}

/// A provider's name as it is shown: an acronym stays one, anything else gets
/// one capital.
pub fn provider_name(name: &str) -> String {
    // Keep the built-in provider name as its owner writes it.
    match name {
        "dns-ai.ru" => return "DNS-AI.ru".to_string(),
        _ => {}
    }
    let lead: String = name.chars().take_while(|c| c.is_alphabetic()).collect();
    if lead.eq_ignore_ascii_case("dns") {
        return name.to_uppercase();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The master switch of the bypass: what it is called and what it does.
pub const BYPASS_TEXT: (&str, &str) = (
    "Обход через DNS",
    "Настраивает DNS для серверов Google через выбранные DNS-серверы.",
);

/// Labels and descriptions for the switches in the terminal.
pub fn switch_text(cap: Cap) -> (&'static str, &'static str) {
    match cap {
        Cap::ClientPatch => (
            "Вход в аккаунт из-под санкций",
            "Снимает блокировку входа в Google-аккаунт из санкционного региона.",
        ),
        Cap::Watchdog => (
            "Автопатч",
            "Сам накладывает патч на найденный Antigravity — сразу после установки и после каждого обновления, которое его стирает.",
        ),
        Cap::Dns => (
            "Обход через DNS",
            "Служба на этом компьютере отвечает на имена двух серверов Google и держит их \
             разрешающимися через сервисы разблокировки. Без неё обход не работает.",
        ),
        Cap::DnsRotation => (
            "Ротация между серверами",
            "Включено: запрос идёт ко всем включённым серверам, ответ сверяется \
             с эталонным резолвером. Выключено: используется только первый \
             включённый в списке, запасных не будет.",
        ),
    }
}

/// The card for a listener the relay could not bind, by what stopped it.
///
/// Every text ends in something the user can do; the relay retries each minute,
/// so the fixes that need no restart say so.
fn blocked(b: &crate::gate::Blocker, exe: &str) -> Headline {
    let port = b.addr.rsplit(':').next().unwrap_or(&b.addr);
    let effect = "DNS-обход не работает";
    // Port 53 held by a service is the one case where the image name is worse
    // than useless: it is `svchost.exe`, which the user cannot act on. What
    // actually holds it is nearly always Internet Connection Sharing, started
    // behind the user's back by the Hyper-V default switch, the mobile hotspot
    // or a virtual machine - so that is what the card says instead.
    let system_holder = b.by.is_empty() || b.by.eq_ignore_ascii_case("svchost.exe");
    if b.what == "dns" && b.cause == "held" && system_holder {
        return Headline {
            tone: Tone::Action,
            title: format!("Порт {port} занят системной службой"),
            detail: format!(
                "Порт {port} держит системная служба Windows, поэтому {effect}. Обычно это \
                 «Общий доступ к подключению к Интернету (ICS)» — его включают Hyper-V, \
                 мобильный хот-спот и виртуальные машины. Откройте «Службы», остановите \
                 «Общий доступ к подключению к Интернету (ICS)» и поставьте тип запуска \
                 «Отключена». Обход займёт порт сам в течение минуты."
            ),
            action: Some(Action::Repair),
        };
    }
    let (title, detail) = match b.cause.as_str() {
        "held" => {
            let who = if b.by.is_empty() {
                "другая программа".to_string()
            } else {
                format!("программа «{}»", b.by)
            };
            (
                format!("Порт {port} занят другой программой"),
                format!(
                    "Порт {port} на этом компьютере занимает {who}, поэтому {effect}. Закройте её \
                     или отключите в ней DNS-сервер — обход сам займёт порт в течение минуты. \
                     Или нажмите «Починить»."
                ),
            )
        }
        "reserved" => (
            format!("Windows закрыла порт {port}"),
            format!(
                "Порт {port} зарезервирован системой — так делают Hyper-V, WSL и Docker, — \
                 поэтому {effect}. Откройте командную строку от имени администратора, выполните \
                 «net stop winnat», затем «net start winnat» и нажмите «Починить»."
            ),
        ),
        "denied" => (
            "Антивирус блокирует обход".to_string(),
            format!(
                "Антивирус или файрвол не даёт службе обхода открыть порт {port}, поэтому \
                 {effect}. Добавьте в исключения антивируса файл {} — обход заработает сам в \
                 течение минуты. Или нажмите «Починить».",
                exe_or_default(exe)
            ),
        ),
        _ => (
            format!("Порт {port} недоступен"),
            format!(
                "Служба обхода не может открыть порт {port} ({}), поэтому {effect}. Нажмите \
                 «Починить».{}",
                b.error,
                super::report::when_stuck()
            ),
        ),
    };
    Headline {
        tone: Tone::Action,
        title,
        detail,
        action: Some(Action::Repair),
    }
}

/// The relay's exe as it reported it, or where it is installed by default.
fn exe_or_default(exe: &str) -> String {
    if !exe.is_empty() {
        exe.to_string()
    } else {
        r"C:\ProgramData\AGUnlocker\ag_dns.exe".to_string()
    }
}

/// One blocker as a line of the report: what, where, why.
pub fn blocker_line(b: &crate::gate::Blocker) -> String {
    let what = "DNS-релей";
    let why = match b.cause.as_str() {
        "held" if !b.by.is_empty() => format!("порт занят программой «{}»", b.by),
        "held" => "порт занят другой программой".to_string(),
        "reserved" => "порт зарезервирован Windows (Hyper-V/WSL/Docker)".to_string(),
        "denied" => "доступ запрещён — антивирус или файрвол".to_string(),
        _ => "причина не определена".to_string(),
    };
    format!("{what} ({}): {why} — {}", b.addr, b.error)
}

pub fn headline(f: &Facts) -> Headline {
    // Nothing to work on. Said first: every other card assumes Antigravity is
    // installed, and a button that patches nothing is not an action.
    if !f.installs_found && !f.patch_on && !f.bypass_on {
        return Headline {
            tone: Tone::Off,
            title: "Antigravity не найден".into(),
            detail: "Установите Antigravity или укажите папку с ним ниже — карандаш рядом с нужной строкой."
                .into(),
            action: None,
        };
    }

    // Google itself stopped the account until it is verified. Above everything
    // the bypass says: no route and no switch changes this, only the user in a
    // browser - and the client does not always offer the button for it.
    // Only while nothing has answered since: an answer after it means the
    // account was verified, whatever the log still says.
    let verify_due = f
        .verify
        .as_ref()
        .filter(|(ago, _)| *ago <= VERIFY_FOR && f.answer.is_none_or(|a| a > *ago));
    if let Some((ago, _)) = verify_due {
        return Headline {
            tone: Tone::Action,
            title: "Нужна верификация аккаунта Google".into(),
            detail: format!(
                "Google попросил подтвердить аккаунт — {}. Antigravity не всегда показывает \
                 для этого кнопку. Нажмите кнопку ниже: страница Google откроется в браузере \
                 по умолчанию — в нём должен быть выполнен вход в этот же аккаунт. После \
                 подтверждения отправьте сообщение в Antigravity ещё раз.",
                ago_text(*ago)
            ),
            action: Some(Action::Verify),
        };
    }

    // Google errors the bypass cannot fix and must not be blamed for: both mean
    // the request got past the gate. Only while nothing has answered since.
    let unanswered = |ago: Duration| f.answer.is_none_or(|a| a > ago);
    let unauthorized = f
        .unauthorized
        .filter(|(ago, n)| *ago <= crate::gate::RECENT && *n >= UNAUTH_LINES && unanswered(*ago));
    if let Some((ago, _)) = unauthorized {
        return Headline {
            tone: Tone::Action,
            title: "Antigravity нужно перезайти в аккаунт".into(),
            detail: format!(
                "Google отвечает ошибкой 401 — {}: учётные данные Antigravity перестали \
                 совпадать. Выйдите из аккаунта в Antigravity и войдите заново. Обход эту \
                 ошибку не исправит.",
                ago_text(ago)
            ),
            action: None,
        };
    }
    // A refusal newer than the 503 is the news; the 503 was an earlier turn.
    let overloaded = f.overloaded.as_ref().filter(|(ago, _)| {
        *ago <= crate::gate::RECENT && unanswered(*ago) && f.refusal.is_none_or(|(r, _)| r > *ago)
    });
    if let Some((ago, model)) = overloaded {
        let which = if model.is_empty() {
            String::new()
        } else {
            format!(" для модели «{model}»")
        };
        return Headline {
            tone: Tone::Action,
            title: "Серверы Google перегружены".into(),
            detail: format!(
                "Google отвечает ошибкой 503 — {}: нет свободных мощностей{which}. Обход тут ни \
                 при чём — запрос до Google дошёл. Рекомендуется выбрать в Antigravity другую \
                 модель или повторить позже.",
                ago_text(*ago)
            ),
            action: None,
        };
    }

    // The bypass needs an administrator to install and to repair. Asked for
    // only when there is something it would install or repair.
    let service_broken = f.bypass_available && f.bypass_on && (!f.relay_running || f.relay_outdated || !f.rules);
    if f.bypass_available && !f.admin && (!f.bypass_on || service_broken) {
        return Headline {
            tone: Tone::Action,
            title: "Нужны права администратора".into(),
            detail: "Без них обход ошибки 400 не установить и не починить. Разблокировка входа \
                     работает и так."
                .into(),
            action: Some(Action::Elevate),
        };
    }

    // macOS put the install behind App Management and the write is refused: the
    // one reason "Включить всё" keeps leaving the client unpatched. Said with the
    // step that actually fixes it, above the generic "включено не всё" so the
    // button is not a loop. The button stays so a retry after granting is one
    // click away.
    if f.patch_blocked {
        #[cfg(target_os = "macos")]
        let who = crate::macos::app_management_grantee();
        #[cfg(not(target_os = "macos"))]
        let who = "программу, из которой запущен анлокер";
        return Headline {
            tone: Tone::Action,
            title: "macOS не даёт пропатчить Antigravity".into(),
            detail: format!(
                "Antigravity стоит в защищённой папке, и macOS не даёт анлокеру изменить его \
                 файлы. Откройте «Системные настройки» → «Конфиденциальность и безопасность» → \
                 «Управление приложениями» и включите там {who}, затем нажмите «Включить всё»."
            ),
            action: Some(Action::EnableAll),
        };
    }

    if !f.patch_on || (f.bypass_available && !f.bypass_on) {
        let what = match (f.patch_on, f.bypass_on || !f.bypass_available) {
            (false, false) => "Разблокировка входа и обход ошибки 400 выключены.",
            (false, true) => "Разблокировка входа выключена.",
            _ => "Обход ошибки 400 выключен.",
        };
        let closes = if f.patch_on {
            ""
        } else {
            " Antigravity закроется на пару секунд."
        };
        return Headline {
            tone: Tone::Off,
            title: "Включено не всё".into(),
            detail: format!("{what}{closes}"),
            action: Some(Action::EnableAll),
        };
    }

    if service_broken {
        return Headline {
            tone: Tone::Action,
            title: if !f.relay_running {
                "Служба обхода не работает".into()
            } else if f.relay_outdated {
                "Служба обхода устарела".into()
            } else {
                "Служба обхода работает не полностью".into()
            },
            detail: "Без неё ошибка 400 не обходится. Кнопка переустановит и запустит её.".into(),
            action: Some(Action::Repair),
        };
    }

    // A blocked DNS listener names a fixable cause before the generic cut-off verdict.
    if let Some(b) = f.dns_blocked.as_ref().filter(|_| f.bypass_available) {
        return blocked(b, &f.relay_exe);
    }
    if f.bypass_available && f.cut_off {
        match f.net_ok {
            Some(true) => {
                return Headline {
                    tone: Tone::Action,
                    title: "Антивирус или файрвол не пропускает обход".into(),
                    detail: format!(
                        "Служба обхода уже несколько минут не может соединиться ни с одним \
                         сервером, хотя у других программ интернет есть. Так бывает, когда её \
                         блокирует антивирус или файрвол. Добавьте в исключения антивируса и в \
                         разрешённые программы файрвола файл {} и нажмите «Починить».",
                        exe_or_default(&f.relay_exe)
                    ),
                    action: Some(Action::Repair),
                }
            }
            Some(false) => {
                return Headline {
                    tone: Tone::Off,
                    title: "Нет подключения к интернету".into(),
                    detail: "Ни служба обхода, ни анлокер не могут соединиться с серверами в \
                             интернете. Проверьте подключение. Если интернет есть — антивирус \
                             или файрвол мог заблокировать обе программы: добавьте их в \
                             исключения."
                        .into(),
                    action: None,
                }
            }
            None => {}
        }
    }

    // A refusal newer than the newest answer is the live problem.
    let refused_last = match (f.refusal, f.answer) {
        (Some((r, _)), Some(a)) => r < a,
        (Some(_), None) => true,
        _ => false,
    };
    if refused_last {
        let (ago, lines) = f.refusal.unwrap_or_default();
        if !f.bypass_available {
            return Headline {
                tone: Tone::Action,
                title: "Ошибка 400 в Antigravity".into(),
                detail: format!("Google отклонил запрос {}. Проверьте подключение Antigravity и настройки VPN; DNS-обход доступен только в Windows.", ago_text(ago)),
                action: None,
            };
        }
        if ago > REFUSAL_RECENT {
            return Headline {
                tone: Tone::Wait,
                title: "Нужна проверка".into(),
                detail: format!(
                    "Последний раз была ошибка 400 — {}, и после неё модель не отвечала. \
                     Отправьте сообщение в Antigravity ещё раз; если ошибка повторится, \
                     проверьте DNS-сервер и настройки VPN.",
                    ago_text(ago)
                ),
                action: None,
            };
        }
        if lines >= STUCK_LINES {
            return Headline {
                tone: Tone::Action,
                title: "Ошибка 400 повторяется".into(),
                detail: format!(
                    "Последняя — {}. Проверьте выбранный DNS-сервер. \
                     Если у вас включён VPN, попробуйте сменить сервер \
                     и отправьте сообщение ещё раз.{}",
                    ago_text(ago),
                    super::report::when_stuck()
                ),
                action: None,
            };
        }
        return Headline {
            tone: Tone::Action,
            title: "Ошибка 400 в Antigravity".into(),
            detail: format!("Google отклонил запрос {}. Проверьте DNS-сервер и настройки VPN. Отправьте сообщение ещё раз.", ago_text(ago)),
            action: None,
        };
    }

    if let Some(ago) = f.answer {
        return Headline {
            tone: Tone::Ok,
            title: "Работает".into(),
            detail: format!("Модель ответила {}.", ago_text(ago)),
            action: None,
        };
    }

    if f.bypass_available && !f.relay_reporting {
        return Headline {
            tone: Tone::Wait,
            title: "Служба запускается".into(),
            detail: "Это занимает до минуты. Потом напишите что-нибудь в Antigravity.".into(),
            action: None,
        };
    }

    if !f.bypass_available {
        return Headline {
            tone: Tone::Wait,
            title: "Патч входа включён".into(),
            detail: "Патч применён к найденным установкам Antigravity. DNS-обход доступен только в Windows.".into(),
            action: None,
        };
    }

    Headline {
        tone: Tone::Wait,
        title: "Всё включено".into(),
        detail: "Напишите что-нибудь в чат Antigravity — здесь появится подтверждение, что модель \
                 ответила."
            .into(),
        action: None,
    }
}

/// «только что», «22 секунды назад», «3 минуты назад», «2 часа назад».
pub fn ago_text(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 15 {
        return "только что".to_string();
    }
    if secs < 60 {
        return format!(
            "{} {} назад",
            secs,
            plural(secs, "секунду", "секунды", "секунд")
        );
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!(
            "{} {} назад",
            mins,
            plural(mins, "минуту", "минуты", "минут")
        );
    }
    let hours = mins / 60;
    format!("{} {} назад", hours, plural(hours, "час", "часа", "часов"))
}

/// Russian counts in three forms.
pub fn plural(n: u64, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    if n % 100 / 10 == 1 {
        return many;
    }
    match n % 10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working() -> Facts {
        Facts {
            admin: true,
            installs_found: true,
            patch_on: true,
            bypass_on: true,
            bypass_available: true,
            relay_running: true,
            relay_outdated: false,
            rules: true,
            relay_reporting: true,
            ..Facts::default()
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// macOS App Management refusing the write must not read as a plain "включено
    /// не всё": the card has to name the grant, or the patch button just loops.
    #[test]
    fn an_app_management_block_names_the_fix() {
        let f = Facts {
            patch_blocked: true,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Action);
        assert!(
            h.detail.contains("Управление приложениями"),
            "{}",
            h.detail
        );
        assert_eq!(h.action, Some(Action::EnableAll));
    }

    /// G50, the report that started this: a refusal, then quiet. Quiet is not
    /// proof - the card must not turn green without a model answer after it.
    #[test]
    fn silence_after_a_refusal_is_not_working() {
        let f = Facts {
            refusal: Some((secs(300), 4)),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Action);
        assert!(
            h.detail.contains("Отправьте сообщение ещё раз"),
            "{}",
            h.detail
        );
    }

    #[test]
    fn an_answer_after_the_refusal_is_working() {
        let f = Facts {
            refusal: Some((secs(300), 4)),
            answer: Some(secs(60)),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Ok);
        assert_eq!(h.title, "Работает");
        assert!(h.detail.contains("1 минуту назад"), "{}", h.detail);
    }

    /// Found live: an answer, then a refusal, then quiet. Once the refusal is
    /// older than the fixing window the card must not fall back to the answer
    /// before it and say «Работает».
    #[test]
    fn an_old_refusal_after_the_last_answer_asks_for_a_check_not_a_green_card() {
        let f = Facts {
            refusal: Some((secs(13 * 60), 0)),
            answer: Some(secs(14 * 60)),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Wait);
        assert_eq!(h.title, "Нужна проверка");
        // And an answer after that same refusal is working again.
        let f = Facts {
            refusal: Some((secs(13 * 60), 0)),
            answer: Some(secs(60)),
            ..working()
        };
        assert_eq!(headline(&f).tone, Tone::Ok);
    }

    #[test]
    fn a_refusal_after_the_last_answer_is_the_live_problem() {
        let f = Facts {
            refusal: Some((secs(20), 4)),
            answer: Some(secs(600)),
            ..working()
        };
        assert_eq!(headline(&f).tone, Tone::Action);
    }

    #[test]
    fn set_up_and_unproven_asks_for_a_message_and_claims_nothing() {
        let h = headline(&working());
        assert_eq!(h.tone, Tone::Wait);
        assert!(!h.detail.contains("работает"), "{}", h.detail);
        assert_eq!(h.action, None);
    }

    #[test]
    fn many_refusals_with_no_answer_stop_promising_and_say_what_to_try() {
        let f = Facts {
            refusal: Some((secs(10), STUCK_LINES)),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Action);
        assert_eq!(
            h.detail.contains("Сохранить отчёт"),
            super::super::report::ENABLED,
            "{}",
            h.detail
        );
    }

    #[test]
    fn switched_off_offers_the_one_button_and_warns_when_it_closes_antigravity() {
        let f = Facts {
            patch_on: false,
            bypass_on: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.action, Some(Action::EnableAll));
        assert!(h.detail.contains("закроется"), "{}", h.detail);
        // Only the bypass off: nothing gets closed.
        let f = Facts {
            bypass_on: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.action, Some(Action::EnableAll));
        assert!(!h.detail.contains("закроется"), "{}", h.detail);
    }

    #[test]
    fn without_admin_the_button_is_elevation_but_only_when_there_is_work_for_it() {
        let f = Facts {
            admin: false,
            bypass_on: false,
            ..working()
        };
        assert_eq!(headline(&f).action, Some(Action::Elevate));
        // Everything is installed and running: an unelevated window has nothing
        // to ask for.
        let f = Facts {
            admin: false,
            answer: Some(secs(30)),
            ..working()
        };
        assert_eq!(headline(&f).tone, Tone::Ok);
    }

    #[test]
    fn a_dead_or_old_service_is_repaired_by_one_button() {
        let f = Facts {
            relay_running: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!((h.tone, h.action), (Tone::Action, Some(Action::Repair)));
        let f = Facts {
            relay_outdated: true,
            ..working()
        };
        assert_eq!(headline(&f).title, "Служба обхода устарела");
        // Running, current, and still useless: nothing asks it anything.
        let f = Facts {
            rules: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.action, Some(Action::Repair));
        assert_eq!(h.title, "Служба обхода работает не полностью");
    }

    #[test]
    fn nothing_installed_is_said_without_a_button() {
        let f = Facts {
            installs_found: false,
            patch_on: false,
            bypass_on: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Off);
        assert_eq!(h.action, None);
    }

    #[test]
    fn russian_counts_in_three_forms() {
        let minutes = |n| plural(n, "минуту", "минуты", "минут");
        assert_eq!(minutes(1), "минуту");
        assert_eq!(minutes(2), "минуты");
        assert_eq!(minutes(5), "минут");
        assert_eq!(minutes(11), "минут");
        assert_eq!(minutes(21), "минуту");
        assert_eq!(minutes(22), "минуты");
        assert_eq!(minutes(0), "минут");
    }

    #[test]
    fn an_age_reads_as_a_person_would_say_it() {
        assert_eq!(ago_text(secs(3)), "только что");
        assert_eq!(ago_text(secs(22)), "22 секунды назад");
        assert_eq!(ago_text(secs(61)), "1 минуту назад");
        assert_eq!(ago_text(secs(9 * 60)), "9 минут назад");
        assert_eq!(ago_text(secs(2 * 3600 + 5)), "2 часа назад");
        assert_eq!(ago_text(secs(5 * 3600)), "5 часов назад");
    }

    #[test]
    fn an_acronym_stays_an_acronym_and_everything_else_gets_one_capital() {
        assert_eq!(provider_name("dns-ai.ru"), "DNS-AI.ru");
        assert_eq!(provider_name("свой DNS"), "Свой DNS");
        assert_eq!(provider_name("dns.example"), "DNS.EXAMPLE");
        // Must not panic on a name the pool could grow later.
        assert_eq!(provider_name(""), "");
        assert_eq!(provider_name("1.1.1.1"), "1.1.1.1");
    }

    fn blocker(what: &str, cause: &str, by: &str) -> crate::gate::Blocker {
        crate::gate::Blocker {
            what: what.into(),
            addr: "127.0.0.53:53".into(),
            cause: cause.into(),
            by: by.into(),
            error: "os error 10013".into(),
        }
    }

    /// A DNS listener the relay cannot bind is said with the fix, even while the
    /// model still answers through what is left of the bypass.
    #[test]
    fn a_blocked_dns_listener_says_what_to_add_to_the_antivirus() {
        let f = Facts {
            answer: Some(secs(5)),
            dns_blocked: Some(blocker("dns", "denied", "")),
            relay_exe: r"C:\ProgramData\AGUnlocker\ag_dns.exe".into(),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.tone, Tone::Action);
        assert_eq!(h.title, "Антивирус блокирует обход");
        assert!(
            h.detail.contains(r"C:\ProgramData\AGUnlocker\ag_dns.exe"),
            "{}",
            h.detail
        );
        assert!(h.detail.contains("53"), "{}", h.detail);
        assert_eq!(h.action, Some(Action::Repair));
    }

    #[test]
    fn unavailable_dns_does_not_request_a_missing_service() {
        let f = Facts {
            bypass_available: false,
            bypass_on: false,
            relay_running: false,
            relay_reporting: false,
            rules: false,
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.title, "Патч входа включён");
        assert_eq!(h.action, None);
        let refused = headline(&Facts { refusal: Some((secs(5), 4)), ..f });
        assert_eq!(refused.title, "Ошибка 400 в Antigravity");
        assert_eq!(refused.action, None);
    }

    #[test]
    fn a_reserved_dns_port_asks_for_winnat_to_be_restarted() {
        let f = Facts {
            dns_blocked: Some(blocker("dns", "reserved", "")),
            ..working()
        };
        let h = headline(&f);
        assert_eq!(h.title, "Windows закрыла порт 53");
        assert!(h.detail.contains("net stop winnat"));
    }

    /// A relay nothing answers: blocked if this window gets out, offline if it
    /// does not, and no verdict before the window has asked.
    #[test]
    fn a_cut_off_relay_is_told_apart_from_a_machine_with_no_internet() {
        let blocked = Facts {
            cut_off: true,
            net_ok: Some(true),
            ..working()
        };
        let h = headline(&blocked);
        assert_eq!(h.title, "Антивирус или файрвол не пропускает обход");
        assert_eq!(h.action, Some(Action::Repair));
        let offline = Facts {
            cut_off: true,
            net_ok: Some(false),
            ..working()
        };
        assert_eq!(headline(&offline).title, "Нет подключения к интернету");
        let unasked = Facts {
            cut_off: true,
            net_ok: None,
            ..working()
        };
        assert_eq!(headline(&unasked).title, "Всё включено");
    }

    /// Google stopped the account: the card says so above everything, red, with
    /// the one button that helps - even with the bypass switched off, which has
    /// nothing to do with it.
    #[test]
    fn a_verification_demand_tops_the_card_until_an_answer_follows() {
        let url = "https://accounts.google.com/signin/continue?a=1&b=2".to_string();
        let asked = Facts {
            verify: Some((secs(120), url.clone())),
            answer: Some(secs(900)),
            bypass_on: false,
            ..working()
        };
        let h = headline(&asked);
        assert_eq!(h.tone, Tone::Action);
        assert_eq!(h.action, Some(Action::Verify));

        let answered_since = Facts {
            verify: Some((secs(120), url.clone())),
            answer: Some(secs(30)),
            ..working()
        };
        assert_ne!(headline(&answered_since).action, Some(Action::Verify));

        let stale = Facts {
            verify: Some((VERIFY_FOR + secs(1), url)),
            ..working()
        };
        assert_ne!(headline(&stale).action, Some(Action::Verify));
    }

    /// A 503 is Google out of capacity, not the bypass: the card says to pick
    /// another model, and lets go once a model answers or a 400 comes after it.
    #[test]
    fn a_503_asks_for_another_model_until_an_answer_follows() {
        let busy = Facts {
            overloaded: Some((secs(60), "gemini-3.8-flash-high".into())),
            answer: Some(secs(900)),
            ..working()
        };
        let h = headline(&busy);
        assert_eq!(h.title, "Серверы Google перегружены");
        assert!(h.detail.contains("gemini-3.8-flash-high"), "{}", h.detail);
        assert!(h.detail.contains("другую"), "{}", h.detail);

        let answered = Facts {
            overloaded: Some((secs(60), String::new())),
            answer: Some(secs(10)),
            ..working()
        };
        assert_ne!(headline(&answered).title, "Серверы Google перегружены");

        let refused_after = Facts {
            overloaded: Some((secs(300), String::new())),
            refusal: Some((secs(30), 4)),
            ..working()
        };
        assert_ne!(headline(&refused_after).title, "Серверы Google перегружены");
    }

    /// A 401 that keeps coming is a credential Google no longer accepts: the
    /// card asks for a re-login; a single one is a token refreshed late.
    #[test]
    fn repeated_401_asks_to_sign_in_again() {
        let bad = Facts {
            unauthorized: Some((secs(30), 12)),
            ..working()
        };
        let h = headline(&bad);
        assert_eq!(h.title, "Antigravity нужно перезайти в аккаунт");
        assert!(h.detail.contains("Выйдите"), "{}", h.detail);

        let once = Facts {
            unauthorized: Some((secs(30), 1)),
            ..working()
        };
        assert_ne!(headline(&once).title, h.title);

        let answered = Facts {
            unauthorized: Some((secs(30), 12)),
            answer: Some(secs(5)),
            ..working()
        };
        assert_ne!(headline(&answered).title, h.title);
    }
}
