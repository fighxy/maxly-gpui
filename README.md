# maxly-gpui

Неофициальный клиент [MAX](https://max.ru) для Windows. Переписка идёт с `api2.oneme.ru` по протоколу десктопного клиента, телеметрия не отправляется. Окно рисует GPUI.

Сессия представляется сборкой MAX Desktop для Windows 26.41.0.83760. Вход и база остаются на этом компьютере: база зашифрована SQLCipher, ключ базы и токен входа лежат в Диспетчере учётных данных Windows. Для входа лучше отдельный тестовый аккаунт.

Клиент в работе. Уже есть вход по QR и по номеру, список чатов, текст, фото и файлы, реакции и анимированные эмодзи. Звонков, папок и большей части экранов официального клиента ещё нет. Карта функций — `rust/MAP.md`.

## Стек

- Rust 1.85, edition 2024
- Интерфейс: [GPUI](https://github.com/zed-industries/zed) и [gpui-kit](https://github.com/longbridge/gpui-kit) 0.10.0. Ядро крутится на своём runtime Tokio
- Протокол: кадры с 10-байтным заголовком, MessagePack, сжатие LZ4
- TLS: rustls. Хосты `*.oneme.ru` доверяют приложенному Russian Trusted Root CA либо корню Mozilla
- Загрузка и скачивание файлов: HTTP через reqwest
- Хранение: SQLCipher (SQLite) и связка ключей Windows

## Крейты

| Крейт | Зачем |
|---|---|
| `maxly-gpui` | Окно и экраны |
| `max-core` | Сессия, вход, чаты, сообщения, медиа |
| `max-proto` | Кадры, опкоды, пакеты |
| `max-transport` | TLS-соединение, ping, ответы по `seq` |
| `max-identity` | Идентификатор устройства, user agent, таблица версии |
| `max-store` | Зашифрованная база и секреты ОС |
| `max-lottie` | Анимированные эмодзи через ThorVG |

## Библиотеки

Версии — прямые зависимости.

Интерфейс: `gpui-kit` 0.10.0, `image` 0.25.9, `qrcode` 0.15.0, `tray-icon` 0.26.1, `winsafe` 0.0.29, `jiff` 1.0.1, `dirs` 7.0.0.

Протокол и данные: `lz4_flex` 0.14.2, `rmpv` 1.3.1, `serde` 1.0, `serde_json` 1.0, `serde_bytes` 0.11.19, `bytes` 1.12.2.

Сеть: `tokio` 1.54.1, `tokio-util` 0.7.20, `rustls` 0.23.46, `tokio-rustls` 0.26.6, `webpki-roots` 1.0.12, `reqwest` 0.13.

Хранение и секреты: `rusqlite` 0.40.2 со сборкой `bundled-sqlcipher-vendored-openssl`, `keyring` 3.6.3, `getrandom` 0.4.3, `zeroize` 1.9.2.

Устройство: `base64` 0.23.1, `iana-time-zone` 0.1.65, `sys-locale` 0.3.2, `winreg` 0.56.0.

Анимация: `thorvg` 0.6.

Служебные: `thiserror` 2.0.21, `tracing` 0.1.44.

## Запуск

Сборка на Windows описана в `BUILDING.md` (нужен Rust MSVC и Perl для OpenSSL внутри SQLCipher).

```sh
cargo run -p maxly-gpui -- --demo        # выдуманные данные, без сети
cargo run -p maxly-gpui                  # api2.oneme.ru
```
