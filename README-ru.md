[English](README.md) · [Русский](README-ru.md)

# Mago

Форк [Mago](https://github.com/carthage-software/mago) с нативным анализом исключений на Rust и сопровождением PHPDoc `@throws`.

Общие инструменты и настройки описаны в [документации upstream](https://mago.carthage.software/latest/en/) и [его README](https://github.com/carthage-software/mago/blob/main/README.md).

## Возможности форка

- Вычисление исключений из тел функций, методов и замыканий, межфайловых вызовов, callbacks и генераторов.
- Учёт условий аргументов, catch/rethrow/finally и разрешимых Yii 2 hooks.
- Добавление, сужение и удаление `@throws` с сохранением остальных частей PHPDoc.
- Постоянный кэш, отбор диагностик и правок по Git diff, объяснение происхождения исключений.

Обычный `analyze` также проверяет типы, методы, свойства и другие ошибки PHP.

## Установка

```sh
composer config repositories.teletype-mago vcs https://github.com/Teletype-App/mago
composer require --dev 'teletype/mago:^1.51'
vendor/bin/mago --version
```

PHP launcher скачивает готовый бинарник из [релизов форка](https://github.com/Teletype-App/mago/releases). Rust для установки через Composer не нужен. `composer update teletype/mago` обновляет пакет, `composer install` следует lock-файлу. Бинарники доступны для Linux, macOS и Windows.

## Анализ исключений

```toml
[analyzer]
check-throws = true
# plugins = ["yii2"] # Для проектов на Yii 2
```

```sh
vendor/bin/mago analyze --throws-only --throws-cache .mago/throws.json
vendor/bin/mago analyze --throws-only --throws-diff main --fix --potentially-unsafe
```

Вместо `main` укажите свою базовую ветку. Git-отбор ограничивает отчёт и правки, весь настроенный source-набор участвует в анализе. `--throws-explain` сохраняет объяснения в JSON. PHPDoc-правки требуют `--potentially-unsafe`.

## Ограничения и сопровождение

Динамические вызовы могут остаться неразрешёнными. При неизвестных эффектах существующие `@throws` не удаляются и не сужаются. Полное соответствие Psalm не гарантируется.

Пакет `carthage-software/mago` и установщики upstream устанавливают upstream. Для этого форка используйте `teletype/mago`. Обновление бинарника при Composer-установке выполняйте через Composer.

Правила синхронизации и публикации: [FORK_SYNC.md](FORK_SYNC.md). Лицензии: [MIT](LICENSE-MIT) или [Apache 2.0](LICENSE-APACHE).
