# Antigravity Unlocker 2.35.1 для Mac

Сборка для Mac с Apple Silicon и Intel, macOS 13 или новее.

## Установка

Скачайте [`Psychotic_AG_Fork.dmg`](https://github.com/Breakdownx/psychotic-fork/releases/latest/download/Psychotic_AG_Fork.dmg) из [последнего релиза](https://github.com/Breakdownx/psychotic-fork/releases/latest), откройте его, перетащите **Antigravity Unlocker** на ярлык **Applications** и откройте приложение из «Программ». На вкладке **«Обзор»** нажмите **«Проверить»**, чтобы увидеть состояние установленных клиентов.

Первый запуск, право «Управление приложениями», запуск из Терминала и запасной вариант с `Запустить.command` описаны в [macos/README.md](macos/README.md). Тот же текст лежит внутри образа.

## Сборка

```sh
macos/build.sh
```

Скрипт собирает универсальный бинарник (Apple Silicon и Intel; `--arm64-only` — быстрее, только Apple Silicon) и кладёт в локальную папку `release/` (в git её нет) образ `Psychotic_AG_Fork.dmg`, ZIP с приложением и запасную папку с `.tar.gz`. CI ([.github/workflows/macos.yml](.github/workflows/macos.yml)) прогоняет тесты на каждый push, а по тегу `v*` запускает этот же скрипт и публикует релиз с этими файлами. Тег должен совпадать с версией в `Cargo.toml`.

Подпись Developer ID и нотарификация описаны в [docs/обновление-antigravity.md](docs/обновление-antigravity.md).

## Проверка без изменений

```sh
"/Applications/Antigravity Unlocker.app/Contents/MacOS/ag_unlocker" --status
```

Печатает найденные установки, состояние патча и фоновой службы; ничего не меняет.
