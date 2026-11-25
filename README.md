# Antigravity Unlocker 2.35.1 для Mac

Сборка для Mac с Apple Silicon (серия M), macOS 13 или новее.

## Установка

Откройте `AG_2.35.1_macOS_arm64-local.dmg`, перетащите **Antigravity Unlocker** на ярлык **Applications** и откройте приложение из «Программ». На вкладке **«Обзор»** нажмите **«Проверить»**, чтобы увидеть состояние установленных клиентов.

Первый запуск, право «Управление приложениями», запуск из Терминала и запасной вариант с `Запустить.command` описаны в [macos/README.md](macos/README.md). Тот же текст лежит внутри образа.

## Сборка

```sh
macos/build.sh
```

Скрипт собирает релизный бинарник и кладёт в `release/` образ DMG, ZIP с приложением и запасную папку с `.tar.gz`. CI ([.github/workflows/macos.yml](.github/workflows/macos.yml)) прогоняет тесты на каждый push и запускает этот же скрипт по тегу `v*`. Подпись Developer ID и нотарификация описаны в [docs/обновление-antigravity.md](docs/обновление-antigravity.md).

## Проверка без изменений

```sh
"/Applications/Antigravity Unlocker.app/Contents/MacOS/ag_unlocker" --status
```

Печатает найденные установки, состояние патча и фоновой службы; ничего не меняет.
