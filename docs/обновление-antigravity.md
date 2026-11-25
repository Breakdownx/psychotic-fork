# Что делать, когда выходит новая версия Antigravity

Гайд помогает проверить Desktop, IDE и CLI после обновления. Порядок выпуска самого анлокера приведён в приложении.

## Короткий ответ

При включённом автопатче работающая фоновая служба (watchdog) ждёт, пока файлы обновления перестанут меняться, и повторно применяет патч к знакомой сборке. Если ты отключил патч вручную, служба сохраняет этот выбор. После обновления проверь каждый клиент, которым пользуешься. [Настройки автопатча](../src/settings.rs) · [Поведение фоновой службы](../src/watchdog.rs)

## Как проверить обновление

1. Дождись завершения обновления Antigravity. Открой анлокер и на вкладке **«Обзор»** нажми **«Проверить»**. Дождись окончания проверки.
2. Найди отдельную строку для каждого нужного клиента. Если установка отсутствует, открой **«Установки»**, нажми значок папки с подсказкой **«Указать путь вручную»**, укажи папку нужной установки и повтори проверку в «Обзоре».
3. Для каждой строки со статусом **«Патч применён»** запусти именно этот обновлённый клиент и выполни привычное действие. Например, для Desktop или IDE отправь обычный запрос и дождись ответа. CLI проверь отдельно, если им пользуешься.
4. Считай обновление проверенным для конкретного клиента, когда статус подтверждён, клиент запускается и действие завершается успешно. Если шаг сорвался, запиши название клиента, версии Antigravity и анлокера, ОС, время, действие и точный текст ошибки. [Статусы и кнопки интерфейса](../src/gui/main_view/overview.rs)

### Как читать результат

| Что видно | Что это означает и что делать |
|---|---|
| «Ещё не проверено» | У анлокера пока нет результата для этой установки. Дождись окончания проверки и сверь путь, если статус сохранился. |
| «Патч не подтверждён» | Установка не прошла проверку целиком. Например, после прерванного обновления часть файлов может быть обработана, а часть ещё ждёт. Дождись завершения обновления, повтори проверку и посмотри записи для этой установки. |
| «Патч применён» | Проверку прошли все обнаруженные нативные файлы этой установки. Запуск клиента и ответ на запрос проверяются отдельно. |
| Фоновая служба отложила операцию | Дай текущей операции завершиться и повтори проверку. Если сообщение повторяется, сохрани его вместе со временем. |
| Ошибка чтения или записи | Зафиксируй путь и системную ошибку. Ошибка чтения оставляет состояние файла неизвестным. При ошибке записи на macOS см. раздел «Ошибка записи на macOS». |
| «Эта сборка Antigravity анлокеру ещё не знакома» или «Сигнатура … не найдена» | Сохрани версии и сообщение для сопровождающего. Для повторной проверки используй версию анлокера с подтверждённой поддержкой этой сборки. |

Совпадение строки в одном файле Desktop не подтверждает состояние всей установки, IDE или CLI. Если нужно установить, **какую именно копию файла удалось прочитать**, на macOS можно снять контрольную сумму. Укажи путь к файлу из разбираемой установки.

```bash
shasum -a 256 "/полный/путь/к/файлу"
```

При успехе команда печатает SHA-256 и путь. При отсутствии файла или ошибке чтения она выводит причину и возвращает ненулевой код. Сохрани ошибку как отдельный результат. [Как анлокер оценивает установку](../src/patch_binary.rs)

Если на Mac установлено несколько копий Desktop, посмотреть PID и путь процесса из стандартного бандла можно командой `pgrep -fl "Antigravity.app/Contents/Resources/bin/language_server"`. Версию сверь в самом запущенном клиенте.

### Где сохранить журнал

**«Журнал» → «Скопировать журнал»** сохраняет в буфер записи текущего сеанса. Вставь их в файл до закрытия анлокера или деактивации. При следующем запуске журнал окна начинается заново. [Журнал окна](../src/gui/main_view/journal.rs)

Если обновление шло при закрытом окне, проверь отдельный файл фоновой службы и скопируй строки за время сбоя.

| ОС | Файл |
|---|---|
| Windows | `%LOCALAPPDATA%\AGUnlocker\watchdog.log` |
| macOS и Linux | `$XDG_DATA_HOME/agunlocker/watchdog.log`, если переменная задана и непуста. По умолчанию — `~/.local/share/agunlocker/watchdog.log`. |

Фоновая служба создаёт файл при записи события и ограничивает его размер. [Каталог журналов](../src/dns_forwarder.rs) · [Запись событий watchdog](../src/watchdog.rs)

---

## Ошибка записи на macOS

На macOS патч ложится только если анлокер **может писать** в бандл
`/Applications/Antigravity.app`. Свежеобновлённый из апдейтера бандл macOS
защищает правами **App Management (TCC)**, и анлокер, запущенный двойным кликом
из Finder, этих прав не наследует → запись падает с EPERM, а настольный клиент
остаётся непропатченным (даже когда CLI патчится нормально).

Что сказать пользователю / сделать самому:

- Системные настройки → Конфиденциальность и безопасность → **Управление
  приложениями** → включить **Терминал** (или программу, из которой запускается
  анлокер).
- Либо запускать через `Запустить.command` **из Терминала**, у которого это право
  есть (двойной клик по самому бинарнику репарентит процесс на launchd и теряет
  грант).

Переподпись (ad-hoc, без hardened runtime) и снятие quarantine делаются
автоматически в `write_atomic` / `macos::resign` — тут вручную ничего не надо.

---

## Если нужно вернуться к штатной установке

1. Сохрани работу в Antigravity, скопируй журнал окна и нужные записи фоновой службы. Полная деактивация закрывает Antigravity, снимает патчи, отключает обход и удаляет фоновые службы и данные анлокера. Версия самого Antigravity при этом остаётся установленной.
2. Открой **«Установки» → «Полная деактивация»**. Прочитай диалог и нажми **«Деактивировать»**. В Windows для этого нужны права администратора.
3. Дождись сообщения **«Анлокер полностью деактивирован»**, закрой анлокер и проверь запуск штатного клиента. Если появилось **«Деактивация не завершена»**, скопируй новые строки журнала, устрани указанную причину и повтори деактивацию. [Диалог](../src/gui/main_view/installations.rs) · [Обработка результата](../src/ops.rs)

---

## Что именно трогает патч (что может «уехать»)

Этот раздел описывает пять частей реализации, которые зависят от устройства конкретной версии Antigravity.

### 1. Переименования в нативных бинарниках (главное)

Патч — это две **побайтовые замены одинаковой длины** в `language_server` и `agy`:

| из | в | где задано |
|----|----|-----------|
| `ineligible` | `inexigible` | `field_signatures()` — [patch_binary.rs:391](../src/patch_binary.rs) |
| `https_proxy` | `AG_LS_PROXY` | `proxy_var_original()` [:404], `PROXY_VAR_NEW` [:527] |

Критично: **длина должна совпадать** (10 и 11 байт). Весь трюк «переименование без
смещений» держится на равной длине — новый литерал обязан быть такой же длины,
иначе патч так работать не будет.

### 2. Где лежат эти бинарники

`binary_targets()` — [patch_binary.rs:646](../src/patch_binary.rs). Там
перечислены `agy`/`agy.exe`, `resources/bin/language_server(.exe)`, встроенный в
IDE `extensions/antigravity/bin/language_server*` (с глобом по платформенному
суффиксу) и `~/.gemini/bin`. На macOS путь к `Resources` даёт
`macos::resources_dir()`. Если Antigravity **переименует или переместит** бинарник
— правка здесь.

### 3. Архитектура Desktop (JS или нет)

`is_new_desktop_architecture()` — [patch_ide.rs:243](../src/patch_ide.rs).
С версии 2.4+ вся авторизация ушла в `language_server`, а `dist/main.js`
Desktop больше не патчится. Определяется по маркерам `./languageServer` +
`./ipcHandlers` и отсутствию старых `_handleAuthErrorResponse` / `getUserStatus` /
`"AUTH_SUCCESS"` / `"SET_INELIGIBLE"`. Если Google снова поменяет компоновку shell —
проверять и, возможно, чинить эту функцию и `process_install`
([src/main.rs](../src/main.rs)).

### 4. Регекс патча IDE `out/main.js`

`patch_ide()` — [patch_ide.rs:159](../src/patch_ide.rs). Это длинный regex по
функции обработки авторизации. Самое хрупкое к обновлениям IDE: при смене их кода
regex перестаёт матчиться → `SignatureMissing`.

### 5. Регекс патча `extension.js`

`patch_extension_js()` — [patch_ide.rs:635](../src/patch_ide.rs). Косметика рядом
с основным патчем, но тоже завязана на их код.

### Если сломался IDE / extension

Признак — `SignatureMissing` от `repatch_ide_js` / `patch_extension_js`. Открой
обновлённый `out/main.js` (или `extension.js`), найди кусок, который раньше
матчился regex’ом, и подгони regex в [patch_ide.rs](../src/patch_ide.rs) под новый
код. При несовпадении шаблона эта операция оставляет соответствующий JS-файл без изменений. Состояние остальных файлов установки проверяется отдельно.

---

## Приложение. Выпуск Antigravity Unlocker для Mac

Для Finder и отправки через Telegram подготовь ZIP с одним объектом в корне — `Antigravity Unlocker.app`. Внутри приложения находятся бинарник, иконка и метаданные. Получатель распаковывает ZIP в Finder и открывает приложение двойным щелчком. Сборка ниже предназначена для **Apple Silicon (arm64), macOS 13 и новее**. Для Intel потребуется отдельный бинарник x86_64 или Universal.

### 1. Собери бинарник и создай приложение

Выполняй блоки из корня проекта в одном сеансе Bash на Mac с Xcode и Rust. В примере используется версия `2.35.1`. Перед новым выпуском сверь базовую версию пакета `ag_unlocker` в `Cargo.toml` и `Cargo.lock`. Если полная версия — `2.35.1_2`, укажи её в `release_version`, оставь `base_version=2.35.1` и задай отдельный числовой номер сборки в `bundle_build`. Полная версия передаётся при компиляции и видна в заголовке окна. Флаг `--version` печатает базовую версию Cargo. [Cargo.toml](../Cargo.toml) · [Выбор отображаемой версии](../src/update.rs)

```bash
set -eu
release_version=2.35.1
base_version=2.35.1
bundle_build=2.35.1
arch=aarch64-apple-darwin
work=$(mktemp -d)
app="$work/Antigravity Unlocker.app"
export AG_FULL_VERSION="$release_version"
export MACOSX_DEPLOYMENT_TARGET=13.0

cargo test --locked
cargo build --locked --release --bin ag_unlocker --target "$arch" --target-dir target
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 755 "target/$arch/release/ag_unlocker" "$app/Contents/MacOS/ag_unlocker"
sips -s format icns icon.ico --out "$app/Contents/Resources/AppIcon.icns"
```

Создай `Info.plist` до подписи. Finder запускает `Contents/MacOS/ag_unlocker` напрямую.

```bash
python3 - "$app/Contents/Info.plist" "$base_version" "$bundle_build" <<'PY'
import plistlib
import sys

path, version, build = sys.argv[1:]
with open(path, "wb") as out:
    plistlib.dump({
        "CFBundlePackageType": "APPL",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleIdentifier": "local.antigravity.unlocker",
        "CFBundleDevelopmentRegion": "ru",
        "CFBundleName": "Antigravity Unlocker",
        "CFBundleDisplayName": "Antigravity Unlocker",
        "CFBundleExecutable": "ag_unlocker",
        "CFBundleIconFile": "AppIcon.icns",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": build,
        "LSApplicationCategoryType": "public.app-category.utilities",
        "LSMinimumSystemVersion": "13.0",
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
    }, out)
PY
plutil -lint "$app/Contents/Info.plist"
lipo "$app/Contents/MacOS/ag_unlocker" -verify_arch arm64
```

### 2. Выбери способ подписи

**Локальная проверка.** Подпись ad hoc позволяет проверить целостность собранного приложения. Архив помечается `app-local`. Таким способом собран `AG_2.35.1_macOS_arm64_app-local.zip`: проверка целостности прошла, Gatekeeper отклонил распакованное приложение при проверке распространения.

```bash
codesign --force --sign - --options runtime "$app"
zip_name="AG_${release_version}_macOS_arm64_app-local.zip"
```

**Рассылка пользователям.** Подготовь сертификат **Developer ID Application** с закрытым ключом и профиль учётных данных для `notarytool` в связке ключей. Найди точное имя сертификата через `security find-identity -v -p codesigning`. Подставь имя своего сертификата и профиля в блок ниже. Если сертификат или профиль недоступен, сохрани сборку как `app-local`. Команда `submit` отправляет промежуточный ZIP в службу нотарификации Apple. [Процесс нотарификации Apple](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)

```bash
identity="Developer ID Application: ИМЯ (TEAMID)"
notary_profile="ИМЯ_ПРОФИЛЯ"
codesign --force --sign "$identity" --options runtime --timestamp "$app"
ditto -c -k --keepParent "$app" "$work/submit.zip"
xcrun notarytool submit "$work/submit.zip" --keychain-profile "$notary_profile" \
  --wait --no-progress --output-format plist > "$work/notary.plist"
plutil -p "$work/notary.plist"
test "$(plutil -extract status raw -o - "$work/notary.plist")" = Accepted
xcrun stapler staple "$app"
zip_name="AG_${release_version}_macOS_arm64_app.zip"
```

При статусе `Invalid` возьми `id` заявки из `notary.plist` и посмотри причину через `xcrun notarytool log ID --keychain-profile ИМЯ_ПРОФИЛЯ`. После исправления заново подпиши приложение и отправь его на проверку. Финальный ZIP создаётся после прикрепления билета Apple.

### 3. Проверь ZIP, который получит пользователь

Для обоих вариантов подписи упакуй приложение и распакуй архив в пустой каталог. Проверки выполняй на извлечённой копии.

```bash
ditto -c -k --keepParent "$app" "$work/$zip_name"
unzip -tq "$work/$zip_name"
ditto -x -k "$work/$zip_name" "$work/unpacked"
checked="$work/unpacked/Antigravity Unlocker.app"
test -x "$checked/Contents/MacOS/ag_unlocker"
plutil -lint "$checked/Contents/Info.plist"
codesign --verify --deep --strict --verbose=2 "$checked"
```

Для **версии на рассылку** выполни ещё две проверки. Обе команды должны завершиться успешно, а `spctl` — показать `accepted`.

```bash
xcrun stapler validate "$checked"
spctl --assess --type execute --verbose=4 "$checked"
```

Сохрани проверенный ZIP и выведи его SHA-256. Перед рассылкой передай этот архив на другой Mac с Apple Silicon, распакуй в Finder и проверь первый запуск двойным щелчком и версию в окне. При результате Gatekeeper `rejected` вернись к подписи и нотарификации. [Проверка программ в macOS](https://support.apple.com/ru-ru/102445)

```bash
mkdir -p release
install -m 644 "$work/$zip_name" "release/$zip_name"
shasum -a 256 "release/$zip_name"
```

**Прежний комплект с `Запустить.command`.** [build_rust.py](../build_rust.py) на Windows через WSL и [ручной workflow macOS](../.github/workflows/macos.yml) по-прежнему создают папку и `.tar.gz` с бинарником, скриптом запуска и [README](../macos/README.md). Для версии 2.35.1 Python формирует `AG_2.35.1_macos.tar.gz`, workflow — `AG_2.35_1_macos.tar.gz`. Для кросс-сборки через WSL нужны cargo-zigbuild, целевая платформа Rust и macOS SDK. Шаг macOS может быть пропущен, поэтому проверяй вывод текущей сборки. Для ZIP с приложением используй шаги 1–3 выше.

---

## Чек-лист после обновления

- [ ] Обновление Antigravity завершилось. Если ожидалось автоматическое восстановление, проверены настройки автопатча и сообщения фоновой службы.
- [ ] На вкладке «Обзор» нажата кнопка «Проверить», получен отдельный результат для каждого используемого клиента.
- [ ] Для каждой установки с подтверждённым статусом проверены запуск обновлённой версии и привычное действие.
- [ ] Если результат не подтверждён, сохранён точный статус каждой затронутой установки. Отдельно отмечены отложенная операция, ошибка чтения или записи либо сообщение о незнакомой сборке.
- [ ] При сбое сохранены версии, ОС, время, точная ошибка и записи текущего сеанса или фоновой службы.
- [ ] Если потребовалась деактивация, журналы сохранены заранее, получено подтверждение завершения и проверен запуск штатного клиента.
- [ ] Перед выпуском сверены полная и базовая версии, пройдены тесты и проверена подпись приложения из готового ZIP.
- [ ] Перед рассылкой проверены билет нотарификации, результат Gatekeeper `accepted` и первый запуск из Finder на другом Mac с Apple Silicon.
