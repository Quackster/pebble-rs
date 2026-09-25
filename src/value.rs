// Port of the value layer of the external `io.pebbletemplates:pebble` template
// engine (Pebble 3.1.5), implemented inline since the library is not in the
// repo. `PebbleValue` mirrors Pebble's `Literal`/`PebbleVariable` runtime
// values; `PebbleObject` mirrors the dynamic property/method access Pebble
// performs on non-literal context values.
use std::any::Any;

/// A runtime template value (Pebble `Literal` + object reference).
#[derive(Clone)]
pub enum PebbleValue<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PebbleValue<'a>>),
    Map(Vec<(String, PebbleValue<'a>)>),
    Entry(Box<MapEntry<'a>>),
    Obj(&'a dyn PebbleObject),
}

/// A `Map.Entry` (Pebble iterates `entrySet()` results with `getKey()` /
/// `getValue()`).
#[derive(Clone)]
pub struct MapEntry<'a> {
    pub key: PebbleValue<'a>,
    pub value: PebbleValue<'a>,
}

impl PebbleObject for MapEntry<'_> {
    fn prop<'p>(&'p self, name: &str) -> Option<PebbleValue<'p>> {
        match name {
            "key" => Some(self.key.clone()),
            "value" => Some(self.value.clone()),
            _ => None,
        }
    }

    fn call<'p>(&'p self, name: &str, _args: &[PebbleValue<'p>]) -> Option<PebbleValue<'p>> {
        match name {
            "get_key" | "key" => Some(self.key.clone()),
            "get_value" | "value" => Some(self.value.clone()),
            _ => None,
        }
    }
}

/// Dynamic property/method access for non-literal values (Pebble resolves
/// `obj.field` / `obj.method()` against arbitrary Java objects; the Rust
/// equivalent is this trait, resolved by the `name_variants` /
/// `call_variants` candidate lists).
pub trait PebbleObject: Send + Sync {
    fn prop<'p>(&'p self, name: &str) -> Option<PebbleValue<'p>>;
    fn call<'p>(&'p self, name: &str, args: &[PebbleValue<'p>]) -> Option<PebbleValue<'p>>;
}

/// Converts a template-store value into a `PebbleValue`. The Java
/// `TemplateBinder` values (arbitrary Java objects) map to this trait.
pub trait ToPebble: Send {
    fn to_pebble(&self) -> Option<PebbleValue<'_>>;
}

impl ToPebble for &str {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Str(self.to_string()))
    }
}

impl<T: ToPebble + Sync + ?Sized> ToPebble for &T {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        (**self).to_pebble()
    }
}

/// An erased, owned template value (an entry in the Java `Template` value
/// bag); carries the `Any` downcast target for `Template::get`.
pub trait TemplateValue: Send {
    fn to_pebble(&self) -> Option<PebbleValue<'_>>;
    fn as_any(&self) -> &dyn Any;
}

impl<T: ToPebble + Any> TemplateValue for T {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        ToPebble::to_pebble(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ToPebble for String {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Str(self.clone()))
    }
}

impl ToPebble for i32 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for i64 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self))
    }
}

impl ToPebble for i16 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for u8 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for u16 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for u32 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for usize {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Int(*self as i64))
    }
}

impl ToPebble for f64 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Float(*self))
    }
}

impl ToPebble for f32 {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Float(*self as f64))
    }
}

impl ToPebble for bool {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Bool(*self))
    }
}

impl ToPebble for () {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::Null)
    }
}

impl<T: ToPebble> ToPebble for Option<T> {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        match self {
            Some(t) => t.to_pebble().or(Some(PebbleValue::Null)),
            None => Some(PebbleValue::Null),
        }
    }
}

impl<T: ToPebble> ToPebble for Vec<T> {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::List(
            self.iter().map(|v| v.to_pebble().unwrap_or(PebbleValue::Null)).collect(),
        ))
    }
}

impl<K: ToPebble, V: ToPebble> ToPebble for std::collections::HashMap<K, V> {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        map_to_pebble(self.iter())
    }
}

impl<K: ToPebble, V: ToPebble> ToPebble for std::collections::BTreeMap<K, V> {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        map_to_pebble(self.iter())
    }
}

/// Shared `ToPebble` body for the map types (iteration order is preserved, so
/// `BTreeMap` renders in sorted-key order).
fn map_to_pebble<'a, K: ToPebble + 'a, V: ToPebble + 'a>(
    iter: impl Iterator<Item = (&'a K, &'a V)>,
) -> Option<PebbleValue<'a>> {
    Some(PebbleValue::Map(
        iter.map(|(k, v)| (key_string(k.to_pebble().as_ref()), v.to_pebble().unwrap_or(PebbleValue::Null)))
            .collect(),
    ))
}

/// The string form of a map key (Pebble map keys are rendered as strings).
pub fn key_string(v: Option<&PebbleValue<'_>>) -> String {
    match v {
        Some(PebbleValue::Str(s)) => s.clone(),
        Some(PebbleValue::Int(i)) => i.to_string(),
        Some(PebbleValue::Float(f)) => f.to_string(),
        Some(PebbleValue::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

impl<A: ToPebble, B: ToPebble> ToPebble for (A, B) {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::List(vec![
            self.0.to_pebble().unwrap_or(PebbleValue::Null),
            self.1.to_pebble().unwrap_or(PebbleValue::Null),
        ]))
    }
}

impl<A: ToPebble, B: ToPebble, C: ToPebble> ToPebble for (A, B, C) {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(PebbleValue::List(vec![
            self.0.to_pebble().unwrap_or(PebbleValue::Null),
            self.1.to_pebble().unwrap_or(PebbleValue::Null),
            self.2.to_pebble().unwrap_or(PebbleValue::Null),
        ]))
    }
}

impl ToPebble for serde_json::Value {
    fn to_pebble(&self) -> Option<PebbleValue<'_>> {
        Some(json_to_pebble(self))
    }
}

pub fn json_to_pebble(v: &serde_json::Value) -> PebbleValue<'_> {
    use serde_json::Value as J;
    match v {
        J::Null => PebbleValue::Null,
        J::Bool(b) => PebbleValue::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                PebbleValue::Int(i)
            } else {
                PebbleValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        J::String(s) => PebbleValue::Str(s.clone()),
        J::Array(items) => PebbleValue::List(items.iter().map(json_to_pebble).collect()),
        J::Object(map) => PebbleValue::Map(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_pebble(v)))
                .collect(),
        ),
    }
}

/// `camelCase` -> `snake_case` (Pebble template names are camelCase; the Rust
/// object adapters expose snake_case names).
pub fn camel_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 && prev_lower {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev_lower = c.is_ascii_lowercase() || c == '_';
    }
    out
}

/// Property-access name candidates in try order for a template name.
pub fn name_variants(name: &str) -> Vec<String> {
    let snake = camel_to_snake(name);
    let lower = name.to_lowercase();
    let mut out = vec![name.to_string()];
    if snake != name {
        out.push(snake);
    }
    if lower != name && !out.contains(&lower) {
        out.push(lower);
    }
    out
}

/// Method-call name candidates in try order (`getId` also resolves to `id`).
pub fn call_variants(name: &str) -> Vec<String> {
    let snake = camel_to_snake(name);
    let mut out = Vec::new();
    let push = |out: &mut Vec<String>, s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    push(&mut out, name.to_string());
    push(&mut out, snake.clone());
    for prefix in ["get", "is", "set"] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            if !stripped.is_empty() {
                push(&mut out, stripped.to_string());
                push(&mut out, camel_to_snake(stripped));
            }
        }
    }
    push(&mut out, name.to_lowercase());
    out
}

/// HTML escaping (`Pebble` `autoescape` `html` mode / `escape` filter).
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// JavaScript string escaping (Pebble `escape('js')`).
pub fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}
