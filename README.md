# Psychotic Antigravity Unlock

![Обложка проекта Psychotic Antigravity Unlock](assets/psychotic-antigravity-unlock.png)

Форк Antigravity Unlocker для Mac. Приложение находит установленные Antigravity Desktop, IDE и CLI, показывает состояние патча для каждого клиента и может восстанавливать его после обновлений. Встроенный локальный HTTP-прокси и настройка своего HTTP-прокси удалены. Своим VPN вы управляете отдельно.

## Установка на Mac

1. Скачайте [Psychotic_AG_Fork.dmg](https://github.com/Breakdownx/psychotic-fork/releases/latest/download/Psychotic_AG_Fork.dmg).
2. Откройте образ, перетащите **Antigravity Unlocker** на ярлык **Applications** и запустите приложение из «Программ».
3. На вкладке **«Обзор»** нажмите **«Проверить»**. У Desktop, IDE и CLI отдельные строки с результатом. После обновления запустите нужный клиент и проверьте привычное действие.

Образ рассчитан на Apple Silicon и Intel, начиная с macOS 13. В том же релизе есть ZIP с приложением и архив для запуска из Терминала. [Все файлы релиза](https://github.com/Breakdownx/psychotic-fork/releases/latest)

## Диагностика

Чтобы получить отчёт без запуска окна, выполните в Терминале команду

```sh
"/Applications/Antigravity Unlocker.app/Contents/MacOS/ag_unlocker" --status
```

Отчёт показывает состояние файлов каждой установки, настройку автопатча и состояние фоновой службы. Отчёт можно получить без изменения настроек и бинарников клиентов. На macOS для проверки права записи она может создать временный файл внутри проверяемого приложения.

## Документация

- [Руководство для macOS](macos/README.md) — форматы поставки, первый запуск, разрешения и коды выхода `--status`.
- [После обновления Antigravity](docs/обновление-antigravity.md) — значения статусов, журналы и возврат к штатной установке.
- [Руководство для Linux](linux/README.md) — запуск бета-порта из исходников или готового Linux-комплекта.
- [Сборка и выпуск для Mac](docs/обновление-antigravity.md#сборка-и-выпуск-для-mac) — локальная сборка, подпись и GitHub Actions.

Исходный проект — [Antigravity Unlocker](https://github.com/confeden/Antigravity). Условия использования приведены в [лицензии](LICENSE).
