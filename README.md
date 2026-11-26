# Psychotic Antigravity Unlock

![Обложка проекта Psychotic Antigravity Unlock](assets/psychotic-antigravity-unlock.png)

Форк Antigravity Unlocker для Mac и Windows. Приложение находит установленные Antigravity Desktop, IDE и CLI, показывает состояние патча для каждого клиента и может восстанавливать его после обновлений. Встроенный локальный HTTP-прокси и настройка своего HTTP-прокси удалены. Своим VPN вы управляете отдельно.

## Установка на Mac

1. Скачайте [Psychotic_AG_Fork.dmg](https://github.com/Breakdownx/psychotic-fork/releases/latest/download/Psychotic_AG_Fork.dmg).
2. Откройте образ, перетащите **Antigravity Unlocker** на ярлык **Applications** и запустите приложение из «Программ».
3. На вкладке **«Обзор»** нажмите **«Проверить»**. У Desktop, IDE и CLI отдельные строки с результатом. После обновления запустите нужный клиент и проверьте привычное действие.

Образ рассчитан на Apple Silicon и Intel, начиная с macOS 13. В том же релизе есть ZIP с приложением и архив для запуска из Терминала. [Все файлы релиза](https://github.com/Breakdownx/psychotic-fork/releases/latest)

## Диагностика

Для Windows см. [как получить отчёт в PowerShell, найти журнал и восстановить штатную установку](docs/windows.md).

На Mac откройте Терминал и выполните команду

```sh
"/Applications/Antigravity Unlocker.app/Contents/MacOS/ag_unlocker" --status
```

Отчёт показывает состояние файлов каждой установки, настройку автопатча и состояние фоновой службы. Отчёт можно получить без изменения настроек и бинарников клиентов. На macOS для проверки права записи она может создать временный файл внутри проверяемого приложения.

## Документация

- [Руководство для macOS](macos/README.md) — форматы поставки, первый запуск, разрешения и коды выхода `--status`.
- [После обновления Antigravity](docs/обновление-antigravity.md) — значения статусов, журналы и возврат к штатной установке.
- [Руководство для Linux](linux/README.md) — запуск бета-порта из исходников или готового Linux-комплекта.
- [Руководство для Windows](docs/windows.md) — отчёт `--status` в PowerShell, журнал и возврат к штатной установке.
- [Сборка и выпуск для Mac](docs/обновление-antigravity.md#сборка-и-выпуск-для-mac) — локальная сборка, подпись и GitHub Actions.
- [Сборка для Windows](docs/обновление-antigravity.md#сборка-для-windows) — локальная сборка EXE и workflow Windows.

Исходный проект — [Antigravity Unlocker](https://github.com/confeden/Antigravity). Условия использования приведены в [лицензии](LICENSE).

## Windows

Каждый релиз форка содержит пакеты для Mac и EXE для Windows.

1. Откройте [последний релиз](https://github.com/Breakdownx/psychotic-fork/releases/latest) и скачайте `AG_<версия>.exe`, например `AG_2.35.2.exe`.
2. Запустите файл. Отдельная установка не требуется.
3. На вкладке **«Обзор»** нажмите **«Проверить»** и убедитесь, что у нужного клиента есть строка с результатом.

EXE собран для x86-64 и Windows 10 или новее, Visual C++ Redistributable ему не нужен. На Windows для ARM он работает в режиме эмуляции. Файл не подписан, поэтому SmartScreen может показать окно «Windows защитила ваш компьютер». Чтобы запустить файл, нажмите **«Подробнее» → «Выполнить в любом случае»**. Отчёт в PowerShell, журнал и возврат к штатной установке описаны в [руководстве для Windows](docs/windows.md).

Сборку exe для любой ветки или pull request можно скачать без релиза. Откройте [запуски workflow Windows](https://github.com/Breakdownx/psychotic-fork/actions/workflows/windows.yml), выберите успешный запуск и в разделе **Artifacts** скачайте архив `AG_<версия>_windows`. Для скачивания нужен вход в GitHub. Артефакты хранятся по умолчанию 90 дней.
