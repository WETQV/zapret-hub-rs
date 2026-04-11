# Architecture

## Direction

The app should not depend on random `.cmd` files as its main logic forever.
For the first stage it may call existing scripts, but the long-term center of control should live in Rust.

## App layers

### 1. Core

Responsible for:

- path discovery
- config loading and saving
- process execution
- status checks
- service operations

Planned modules:

- `core::paths`
- `core::config`
- `core::process`
- `core::service`
- `core::status`

### 2. Zapret adapters

A thin layer over the current bundle layout.

Planned modules:

- `zapret::profiles`
- `zapret::telegram_proxy`
- `zapret::lists`
- `zapret::bundle`

Responsibilities:

- map profile IDs to concrete commands
- read and write `list-general-user.txt`
- manage `game_filter.enabled`
- manage `ipset-all.txt`

### 3. UI

Native desktop UI on top of `egui/eframe`.

Planned screens:

- Home
- Profiles
- Service
- Telegram
- Lists
- Logs

## File layout

Planned project layout:

```text
zapret-hub-rs/
  src/
    main.rs
    app.rs
    core/
      mod.rs
      config.rs
      paths.rs
      process.rs
      service.rs
      status.rs
    zapret/
      mod.rs
      bundle.rs
      lists.rs
      profiles.rs
      telegram_proxy.rs
    ui/
      mod.rs
      home.rs
      profiles.rs
      service.rs
      telegram.rs
      lists.rs
      logs.rs
  docs/
    ARCHITECTURE.md
```

## State model

Minimal app state:

- selected bundle path
- selected profile
- service installed or not
- winws running or not
- telegram proxy running or not
- last operation result
- recent log lines

## MVP rules

- Keep bundle path explicit and editable
- Prefer clear controls over complex automation
- No premium/paywall logic
- No updater in the first milestone
- No embedded strategy editor in the first milestone

## Next coding step

Once Rust is installed:

1. `cargo init`
2. add `eframe`
3. create a single window with a status panel and action buttons
4. wire buttons to existing helper commands
