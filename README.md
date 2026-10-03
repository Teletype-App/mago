[English](README.md) · [Русский](README-ru.md)

# Mago

A fork of [Mago](https://github.com/carthage-software/mago) with native Rust exception analysis and `@throws` PHPDoc maintenance.

General tools and configuration are covered by the [upstream documentation](https://mago.carthage.software/latest/en/) and [upstream README](https://github.com/carthage-software/mago/blob/main/README.md).

## Fork features

- Exception inference from function, method and closure bodies, cross-file calls, callbacks and generators.
- Argument conditions, catch/rethrow/finally and resolvable Yii 2 hooks.
- Adding, narrowing and removing `@throws` while preserving other PHPDoc content.
- Persistent caching, Git diff selection for reports and fixes, and exception origin explanations.

Regular `analyze` also checks types, methods, properties and other PHP errors.

## Installation

```sh
composer config repositories.teletype-mago vcs https://github.com/Teletype-App/mago
composer require --dev 'teletype/mago:^1.51'
vendor/bin/mago --version
```

The PHP launcher downloads a prebuilt binary from [fork releases](https://github.com/Teletype-App/mago/releases). Composer installation does not require Rust. `composer update teletype/mago` updates the package, while `composer install` follows the lock file. Binaries are available for Linux, macOS and Windows.

## Exception analysis

```toml
[analyzer]
check-throws = true
# plugins = ["yii2"] # For Yii 2 projects
```

```sh
vendor/bin/mago analyze --throws-only --throws-cache .mago/throws.json
vendor/bin/mago analyze --throws-only --throws-diff main --fix --potentially-unsafe
```

Replace `main` with your base branch. Git selection limits reports and fixes while the entire configured source set participates in analysis. `--throws-explain` writes JSON explanations. PHPDoc fixes require `--potentially-unsafe`.

## Limits and maintenance

Dynamic calls may remain unresolved. Unknown effects prevent removal or narrowing of existing `@throws` tags. Full Psalm compatibility is not guaranteed.

The `carthage-software/mago` package and upstream installers install upstream. Use `teletype/mago` for this fork. Update Composer-installed binaries through Composer.

Synchronization and release rules: [FORK_SYNC.md](FORK_SYNC.md). Licenses: [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
