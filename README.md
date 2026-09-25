# pebble

[![crates.io](https://img.shields.io/crates/v/pebble-rs.svg)](https://crates.io/crates/pebble-rs)
[![docs.rs](https://docs.rs/pebble-rs/badge.svg)](https://docs.rs/pebble-rs)

Pebble template engine written in Rust.

## Provenance

A Rust port of the Java Pebble template engine (`io.pebbletemplates:pebble`, 3.1.5). It mirrors the Java engine's default configuration:

- `strictVariables(false)` — undefined variables and member access on them render as the empty string
- `autoEscaping(false)` — output is **not** escaped by default; only explicit `{% autoescape %}` blocks and the `escape` filter escape

Java-bean conventions are preserved: `name_variants` / `camel_to_snake` / `call_variants` resolve bean-style property and method names (e.g. `getSiteName` ↔ `site_name`), and `entrySet()` iteration with `getKey()` / `getValue()` works as in Java Pebble.

Licensed under Apache-2.0.

## Quick start

```toml
[dependencies]
pebble = "0.1"
```

```rust
use pebble::Engine;
use std::collections::HashMap;
use std::path::PathBuf;

let engine = Engine::new(PathBuf::from("/opt/pebble-templates"));

let mut store: HashMap<String, Box<dyn pebble::value::TemplateValue>> = HashMap::new();
store.insert("site".to_string(), Box::new("Havana".to_string()));
store.insert("items".to_string(), Box::new(vec![1, 2, 3])); // any ToPebble + Any type

let out: String = engine.evaluate("index.tpl", &store);

// No template file? Render in-memory source instead:
let out2: String = engine.evaluate_source("Hello {{ site }}!", &store);
```

Also available:

- `Engine::shared_with(root)` — a process-wide shared engine (`&'static`, `OnceLock`)
- `evaluate_with_defaults(rel, store, defaults)` — render with a fallback scope for missing keys

## Supported syntax

| Tag | Notes |
| --- | --- |
| `{{ expr }}` | variables, literals, arithmetic, ternary `? :`, `and` / `or`, `==` / `!=` and comparisons, `is present` / `is defined`, member access `obj.field`, method calls `obj.method()`, `entrySet()` iteration via `getKey()` / `getValue()` |
| `{% if %}` | `{% elseif %}` / `{% else %}` |
| `{% for %}` | `{% for i in items %}` … `{% endfor %}`, range syntax `[1..n]` |
| `{% set %}` | `{% set x = expr %}` |
| `{% include %}` | root-relative paths, and `../` / `./` paths resolved against the current template's directory |
| `{% autoescape %}` | `html` / `js` |

| Filter | Aliases |
| --- | --- |
| `upper` | `to_upper_case` |
| `lower` | `to_lower_case` |
| `length` | `size` |
| `keys` | |
| `values` | |
| `entry_set` | |
| `replace` | |
| `escape` | |

## Extending for your types

The library is type-agnostic: it knows nothing about host types. You plug your types in through two traits:

- `ToPebble` — convert a value into a `PebbleValue` (impls exist for `String`, `&str`, the integer/float primitives, `bool`, `Option<T>`, `Vec<T>`, `HashMap` / `BTreeMap`, tuples, `serde_json::Value`, …)
- `PebbleObject` — dynamic property/method access for `{{ obj.field }}` and `{{ obj.method() }}`; names arrive through the Java-bean candidate lists, so both `site_name` and `getSiteName` resolve

```rust
use pebble::{PebbleObject, PebbleValue, ToPebble};

impl ToPebble for MyHostType {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Obj(self))
    }
}

impl PebbleObject for MyHostType {
    fn prop<'p>(&'p self, name: &str) -> Option<PebbleValue<'p>> {
        // name arrives via name_variants: "site_name" and "get_site_name" both resolve
        match name {
            "site_name" | "get_site_name" => Some(PebbleValue::Str(self.site_name.clone())),
            _ => None,
        }
    }

    fn call<'p>(&'p self, name: &str, _args: &[PebbleValue<'p>]) -> Option<PebbleValue<'p>> {
        match name {
            "upper" => Some(PebbleValue::Str(self.site_name.to_uppercase())),
            _ => None,
        }
    }
}
```

Any `T: ToPebble + Any` is automatically a valid `TemplateValue`, so it can be inserted directly into the template store.
