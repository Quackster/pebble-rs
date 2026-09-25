// Port of the runtime of the external `io.pebbletemplates:pebble` template
// engine (Pebble 3.1.5), implemented inline since the library is not in the
// repo. Mirrors the `TwigTemplate` engine settings: `strictVariables(false)`
// (undefined values render as the empty string) and `autoEscaping(false)`
// (only explicit `{% autoescape %}` blocks / `escape` filters escape output).
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};


use super::lexer::tokenize;
use super::parser::{parse_stmts, Expr, Lit, Stmt};
use super::value::{
    call_variants, html_escape, js_escape, name_variants, MapEntry, PebbleValue, TemplateValue,
};

static SHARED: OnceLock<Engine> = OnceLock::new();

/// The engine (Pebble `PebbleEngine`) — holds the template root (Java
/// `FileLoader` prefix) and the compiled-template cache.
pub struct Engine {
    root: PathBuf,
    cache: Mutex<HashMap<String, (i64, Arc<Vec<Stmt>>)>>,
}

impl Engine {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The process-wide shared engine (Java builds one per request with the
    /// configured prefix; the Rust equivalent is process-wide, re-read from
    /// disk when the file changes), initialized with an explicit template
    /// root by the first caller.
    pub fn shared_with(root: PathBuf) -> &'static Engine {
        SHARED.get_or_init(|| Engine::new(root))
    }

    /// Load + compile (Pebble `getTemplate`) with an mtime-based cache.
    fn load(&self, rel: &str) -> Option<Arc<Vec<Stmt>>> {
        self.load_path(&self.root.join(rel))
    }

    fn load_path(&self, path: &std::path::Path) -> Option<Arc<Vec<Stmt>>> {
        let mtime = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(-1);

        let key = path.display().to_string();
        {
            let cache = self.cache.lock().unwrap();
            if let Some((m, stmts)) = cache.get(&key) {
                if *m == mtime {
                    return Some(Arc::clone(stmts));
                }
            }
        }

        let src = fs::read_to_string(path).ok()?;
        let stmts = Arc::new(parse_stmts(&tokenize(&src)));
        self.cache
            .lock()
            .unwrap()
            .insert(key, (mtime, Arc::clone(&stmts)));
        Some(stmts)
    }

    /// Evaluate a view against a context store (Java
    /// `compiledTemplate.evaluate(writer, context)`).
    pub fn evaluate<'s>(
        &self,
        rel: &str,
        store: &'s HashMap<String, Box<dyn TemplateValue>>,
    ) -> String {
        let stmts = match self.load(rel) {
            Some(s) => s,
            None => {
                log::error!("pebble: cannot load template {rel}");
                return String::new();
            }
        };
        self.evaluate_scoped(&stmts, vec![vars(store)], Some(rel.to_string()))
    }

    /// Evaluate a view with a fallback scope for keys absent from `store`
    /// (the dev server seeds the fallback with sample context values).
    pub fn evaluate_with_defaults<'s>(
        &self,
        rel: &str,
        store: &'s HashMap<String, Box<dyn TemplateValue>>,
        defaults: &'s HashMap<String, Box<dyn TemplateValue>>,
    ) -> String {
        let stmts = match self.load(rel) {
            Some(s) => s,
            None => {
                log::error!("pebble: cannot load template {rel}");
                return String::new();
            }
        };
        self.evaluate_scoped(
            &stmts,
            vec![vars(defaults), vars(store)],
            Some(rel.to_string()),
        )
    }

    /// Evaluate an in-memory template source (used by the unit tests).
    pub fn evaluate_source<'s, K: ToString>(
        &self,
        src: &str,
        store: &'s HashMap<K, Box<dyn TemplateValue>>,
    ) -> String {
        self.evaluate_scoped(
            &parse_stmts(&tokenize(src)),
            vec![store.iter().map(|(k, v)| (k.to_string(), v)).collect()],
            None,
        )
    }

    /// Evaluate against a scope chain (the last scope shadows the earlier
    /// ones; `% set %` writes into the last scope).
    fn evaluate_scoped<'s>(
        &self,
        stmts: &Vec<Stmt>,
        scopes: Vec<Vec<(String, &'s Box<dyn TemplateValue>)>>,
        current: Option<String>,
    ) -> String {
        let mut ev = Eval {
            engine: self,
            scopes: scopes
                .into_iter()
                .map(|vars| vars.into_iter().map(|(k, v)| (k, v.to_pebble())).collect())
                .collect(),
            autoescape: Vec::new(),
            base: self.root.clone(),
            current,
        };
        let mut out = String::new();
        ev.eval_block(stmts, &mut out);
        out
    }
}

/// The variable list of a context store.
fn vars<'s>(
    store: &'s HashMap<String, Box<dyn TemplateValue>>,
) -> Vec<(String, &'s Box<dyn TemplateValue>)> {
    store.iter().map(|(k, v)| (k.clone(), v)).collect()
}

/// The per-evaluation state (Pebble `EvaluationContext` scope chain).
struct Eval<'e> {
    engine: &'e Engine,
    scopes: Vec<HashMap<String, Option<PebbleValue<'e>>>>,
    autoescape: Vec<String>,
    /// The loader prefix (Java `FileLoader.setPrefix`).
    base: PathBuf,
    /// Name of the template currently being evaluated (Pebble
    /// `PebbleTemplateImpl.name`), the anchor for relative includes.
    current: Option<String>,
}

impl<'e> Eval<'e> {
    fn resolve(&self, name: &str) -> Option<&PebbleValue<'e>> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return v.as_ref();
            }
        }
        None
    }

    /// The `present` test (the ported `PresentTest` — `getScopeChain()
    /// .containsKey(name)`).
    fn contains(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|s| s.contains_key(name))
    }

    fn current_escape(&self) -> Option<&str> {
        self.autoescape.last().map(|s| s.as_str())
    }

    fn escape(&self, mode: &str, s: &str) -> String {
        match mode {
            "js" => js_escape(s),
            _ => html_escape(s),
        }
    }

    fn display(&self, v: Option<&PebbleValue<'e>>) -> String {
        match v {
            None | Some(PebbleValue::Null) => String::new(),
            Some(PebbleValue::Bool(b)) => b.to_string(),
            Some(PebbleValue::Int(i)) => i.to_string(),
            Some(PebbleValue::Float(f)) => f.to_string(),
            Some(PebbleValue::Str(s)) => s.clone(),
            Some(PebbleValue::List(_))
            | Some(PebbleValue::Map(_))
            | Some(PebbleValue::Entry(_))
            | Some(PebbleValue::Obj(_)) => String::new(),
        }
    }

    fn eval_block(&mut self, stmts: &[Stmt], out: &mut String) {
        for stmt in stmts {
            self.eval_stmt(stmt, out);
        }
    }

    fn eval_stmt(&mut self, stmt: &Stmt, out: &mut String) {
        match stmt {
            Stmt::Text(t) => out.push_str(t),
            Stmt::Expr(e) => {
                let v = self.eval(e);
                let s = self.display(v.as_ref());
                if let Some(mode) = self.current_escape() {
                    out.push_str(&self.escape(mode, &s));
                } else {
                    out.push_str(&s);
                }
            }
            Stmt::If(branches, els) => {
                for (cond, body) in branches {
                    let cond = self.eval(cond);
                    if self.truthy(cond) {
                        self.eval_block(body, out);
                        return;
                    }
                }
                self.eval_block(els, out);
            }
            Stmt::For { target, iterable, body } => {
                let items: Vec<PebbleValue<'e>> = match self.eval(iterable) {
                    Some(PebbleValue::List(items)) => items,
                    Some(PebbleValue::Map(items)) => items
                        .into_iter()
                        .map(|(k, _)| PebbleValue::Str(k))
                        .collect(),
                    Some(PebbleValue::Str(s)) => vec![PebbleValue::Str(s)],
                    _ => Vec::new(),
                };
                for item in items {
                    let mut scope = HashMap::new();
                    scope.insert(target.clone(), Some(item));
                    self.scopes.push(scope);
                    self.eval_block(body, out);
                    self.scopes.pop();
                }
            }
        Stmt::Set { name, value } => {
            let v = self.eval(value).unwrap_or(PebbleValue::Null);
            if let Some(top) = self.scopes.last_mut() {
                top.insert(name.clone(), Some(v));
            }
        }
            Stmt::Include(path_expr) => {
                if let Some(PebbleValue::Str(rel)) = self.eval(path_expr) {
                    // Pebble `PebbleTemplateImpl.resolveRelativePath` + `FileLoader`:
                    // only `../` / `./` paths resolve against the current template
                    // name's directory; every other path is prefix (root) relative.
                    let resolved = self.resolve_include(&rel);
                    let prev = std::mem::replace(&mut self.current, Some(resolved.clone()));
                    if let Some(stmts) = self.engine.load_path(&self.base.join(&resolved)) {
                        self.eval_block(&stmts, out);
                    } else {
                        log::error!("pebble: cannot include {rel}");
                    }
                    self.current = prev;
                }
            }
            Stmt::Autoescape(mode_expr, body) => {
                let mode = match self.eval(mode_expr) {
                    Some(PebbleValue::Str(s)) => s,
                    _ => "html".to_string(),
                };
                self.autoescape.push(mode);
                self.eval_block(body, out);
                self.autoescape.pop();
            }
        }
    }

    /// Pebble `PathUtils.resolveRelativePath` — `../`/`./` paths are resolved
    /// against the current template name's directory, everything else is
    /// returned unchanged (prefix-relative in the `FileLoader`).
    fn resolve_include(&self, rel: &str) -> String {
        if !rel.starts_with("../") && !rel.starts_with("./") {
            return rel.to_string();
        }
        let mut segs: Vec<&str> = self
            .current
            .as_deref()
            .map(|s| s.split('/').filter(|p| !p.is_empty()).collect())
            .unwrap_or_default();
        // The anchor is the template name's directory (the file itself drops).
        segs.pop();
        for seg in rel.split('/') {
            if seg == ".." {
                segs.pop();
            } else if seg != "." && !seg.is_empty() {
                segs.push(seg);
            }
        }
        segs.join("/")
    }

    fn truthy(&self, v: Option<PebbleValue<'e>>) -> bool {
        match v {
            None => false,
            Some(PebbleValue::Bool(b)) => b,
            Some(PebbleValue::Int(i)) => i != 0,
            Some(PebbleValue::Float(f)) => f != 0.0,
            Some(PebbleValue::Str(s)) => !s.is_empty(),
            Some(PebbleValue::List(l)) => !l.is_empty(),
            Some(PebbleValue::Null) => false,
            Some(PebbleValue::Entry(_))
            | Some(PebbleValue::Map(_))
            | Some(PebbleValue::Obj(_)) => true,
        }
    }

    fn num(&self, v: Option<&PebbleValue<'e>>) -> Option<f64> {
        match v {
            Some(PebbleValue::Int(i)) => Some(*i as f64),
            Some(PebbleValue::Float(f)) => Some(*f),
            Some(PebbleValue::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn eval(&mut self, e: &Expr) -> Option<PebbleValue<'e>> {
        match e {
            Expr::Lit(l) => Some(match l {
                Lit::Null => PebbleValue::Null,
                Lit::Bool(b) => PebbleValue::Bool(*b),
                Lit::Int(i) => PebbleValue::Int(*i),
                Lit::Float(f) => PebbleValue::Float(*f),
                Lit::Str(s) => PebbleValue::Str(s.clone()),
            }),
            Expr::Var(name) => self.resolve(name).cloned(),
            Expr::Not(inner) => {
                let inner = self.eval(inner);
                Some(PebbleValue::Bool(!self.truthy(inner)))
            }
            Expr::Binary { op, lhs, rhs } => self.eval_binary(op, lhs, rhs),
            Expr::Test {
                name,
                negated,
                expr,
            } => self.eval_test(name, *negated, expr),
            Expr::Ternary { cond, then, els } => {
                let cond = self.eval(cond);
                if self.truthy(cond) {
                    self.eval(then)
                } else {
                    self.eval(els)
                }
            }
            Expr::ListLit(items) => Some(PebbleValue::List(
                items.iter().map(|i| self.eval(i).unwrap_or(PebbleValue::Null)).collect(),
            )),
            Expr::DictLit(items) => Some(PebbleValue::Map(
                items
                    .iter()
                    .map(|(k, v)| (k.clone(), self.eval(v).unwrap_or(PebbleValue::Null)))
                    .collect(),
            )),
            Expr::Range { start, end } => {
                let start = self.eval(start);
                let end = self.eval(end);
                let a = self.num(start.as_ref()).unwrap_or(0.0) as i64;
                let b = self.num(end.as_ref()).unwrap_or(0.0) as i64;
                let items: Vec<PebbleValue<'e>> = (a..=b).map(PebbleValue::Int).collect();
                Some(PebbleValue::List(items))
            }
            Expr::Index { base, index } => {
                let base = self.eval(base)?;
                let idx = self.eval(index)?;
                match (&base, idx) {
                    (PebbleValue::List(items), PebbleValue::Int(i)) => {
                        items.get(i as usize).cloned()
                    }
                    (PebbleValue::Map(items), PebbleValue::Str(key)) => items
                        .iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, v)| v.clone()),
                    (PebbleValue::Str(s), PebbleValue::Int(i)) => s
                        .chars()
                        .nth(i as usize)
                        .map(|c| PebbleValue::Str(c.to_string())),
                    _ => None,
                }
            }
            Expr::Member { base, name } => self.eval_member(base, name),
            Expr::Call { base, name, args } => self.eval_call(base, name, args),
            Expr::Filter { base, name, args } => self.eval_filter(name, base, args),
        }
    }

    fn eval_binary(
        &mut self,
        op: &str,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Option<PebbleValue<'e>> {
        let l = self.eval(lhs);
        let r = self.eval(rhs);
        match op {
            "or" => Some(PebbleValue::Bool(self.truthy(l) || self.truthy(r))),
            "and" => Some(PebbleValue::Bool(self.truthy(l) && self.truthy(r))),
            "==" | "equals" => {
                let equal = match (&l, &r) {
                    (None, None) => true,
                    (Some(PebbleValue::Null), Some(PebbleValue::Null)) => true,
                    (Some(a), Some(b)) => self.value_eq(a, b),
                    _ => false,
                };
                Some(PebbleValue::Bool(equal))
            }
            "!=" | "not_equals" => {
                let equal = match (&l, &r) {
                    (None, None) => true,
                    (Some(PebbleValue::Null), Some(PebbleValue::Null)) => true,
                    (Some(a), Some(b)) => self.value_eq(a, b),
                    _ => false,
                };
                Some(PebbleValue::Bool(!equal))
            }
            "<" | ">" | "<=" | ">=" => {
                // Numeric compare when both sides are numeric.
                let (a, b) = (self.num(l.as_ref()), self.num(r.as_ref()));
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let result = match op {
                            "<" => a < b,
                            ">" => a > b,
                            "<=" => a <= b,
                            _ => a >= b,
                        };
                        Some(PebbleValue::Bool(result))
                    }
                    _ => {
                        let a = l.as_ref().map(|v| self.display(Some(v)));
                        let b = r.as_ref().map(|v| self.display(Some(v)));
                        match (a.as_deref(), b.as_deref()) {
                            (Some(a), Some(b)) => {
                                let result = match op {
                                    "<" => a < b,
                                    ">" => a > b,
                                    "<=" => a <= b,
                                    _ => a >= b,
                                };
                                Some(PebbleValue::Bool(result))
                            }
                            _ => None,
                        }
                    }
                }
            }
            _ => {
                let (a, b) = (self.num(l.as_ref())?, self.num(r.as_ref())?);
                let result = match op {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" => {
                        if b == 0.0 {
                            f64::NAN
                        } else {
                            a / b
                        }
                    }
                    "%" => a % b,
                    _ => return None,
                };
                if result.fract() == 0.0 && result.abs() < 9.0e15 {
                    Some(PebbleValue::Int(result as i64))
                } else {
                    Some(PebbleValue::Float(result))
                }
            }
        }
    }

    fn value_eq(&self, a: &PebbleValue<'e>, b: &PebbleValue<'e>) -> bool {
        match (a, b) {
            (PebbleValue::Null, PebbleValue::Null) => true,
            (PebbleValue::Bool(x), PebbleValue::Bool(y)) => x == y,
            (PebbleValue::Int(x), PebbleValue::Int(y)) => x == y,
            (PebbleValue::Int(x), PebbleValue::Float(y)) => (*x as f64) == *y,
            (PebbleValue::Float(x), PebbleValue::Int(y)) => *x == (*y as f64),
            (PebbleValue::Float(x), PebbleValue::Float(y)) => x == y,
            (PebbleValue::Str(x), PebbleValue::Str(y)) => x == y,
            _ => self.num(Some(a)) == self.num(Some(b))
                && self.num(Some(a)).is_some(),
        }
    }

    fn eval_test(
        &mut self,
        name: &str,
        negated: bool,
        expr: &Expr,
    ) -> Option<PebbleValue<'e>> {
        let v = self.eval(expr);
        let result = match name {
            "present" => match &v {
                Some(PebbleValue::Str(s)) => self.contains(s),
                _ => false,
            },
            "defined" => matches!(
                v.as_ref(),
                Some(v) if !matches!(v, PebbleValue::Null)
            ),
            _ => false,
        };
        Some(PebbleValue::Bool(if negated { !result } else { result }))
    }

    fn eval_member(&mut self, base: &Expr, name: &str) -> Option<PebbleValue<'e>> {
        let base = self.eval(base)?;
        match base {
            PebbleValue::Map(items) => name_variants(name)
                .into_iter()
                .find_map(|n| items.iter().find(|(k, _)| k == &n).map(|(_, v)| v.clone())),
            PebbleValue::List(items) => name
                .parse::<i64>()
                .ok()
                .and_then(|i| items.get(i as usize).cloned()),
            PebbleValue::Entry(entry) => {
                let name = name_variants(name);
                if name.iter().any(|n| n == "key" || n == "get_key") {
                    Some(entry.key.clone())
                } else if name.iter().any(|n| n == "value" || n == "get_value") {
                    Some(entry.value.clone())
                } else {
                    None
                }
            }
            PebbleValue::Obj(obj) => name_variants(name)
                .into_iter()
                .find_map(|n| obj.prop(&n)),
            _ => None,
        }
    }

    fn eval_call(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[Expr],
    ) -> Option<PebbleValue<'e>> {
        let base = self.eval(base)?;
        let arg_values: Vec<PebbleValue<'e>> = args
            .iter()
            .map(|a| self.eval(a).unwrap_or(PebbleValue::Null))
            .collect();
        match base {
            PebbleValue::Str(s) => {
                let snake = super::value::camel_to_snake(name);
                match snake.as_str() {
                    "to_lower_case" => Some(PebbleValue::Str(s.to_lowercase())),
                    "to_upper_case" => Some(PebbleValue::Str(s.to_uppercase())),
                    "size" | "length" => Some(PebbleValue::Int(s.chars().count() as i64)),
                    _ => None,
                }
            }
            PebbleValue::List(items) => {
                let snake = super::value::camel_to_snake(name);
                match snake.as_str() {
                    "size" | "length" => Some(PebbleValue::Int(items.len() as i64)),
                    _ => None,
                }
            }
            PebbleValue::Map(items) => {
                let snake = super::value::camel_to_snake(name);
                match snake.as_str() {
                    "size" | "length" => Some(PebbleValue::Int(items.len() as i64)),
                    "keys" => Some(PebbleValue::List(
                        items.iter().map(|(k, _)| PebbleValue::Str(k.clone())).collect(),
                    )),
                    "values" => Some(PebbleValue::List(
                        items.iter().map(|(_, v)| v.clone()).collect(),
                    )),
                    "entry_set" => self.entries(&items),
                    _ => None,
                }
            }
            PebbleValue::Entry(entry) => {
                let name = call_variants(name);
                if name.iter().any(|n| n == "get_key" || n == "key") {
                    Some(entry.key.clone())
                } else if name.iter().any(|n| n == "get_value" || n == "value") {
                    Some(entry.value.clone())
                } else {
                    None
                }
            }
            PebbleValue::Obj(obj) => call_variants(name)
                .into_iter()
                .find_map(|n| obj.call(&n, &arg_values)),
            _ => None,
        }
    }

    /// `map.entrySet()` — a list of `MapEntry` objects (Java `Map.Entry` with
    /// `getKey()` / `getValue()`).
    fn entries(&self, items: &[(String, PebbleValue<'e>)]) -> Option<PebbleValue<'e>> {
        let list = items
            .iter()
            .map(|(key, value)| PebbleValue::Entry(Box::new(MapEntry {
                key: PebbleValue::Str(key.clone()),
                value: value.clone(),
            })))
            .collect();
        Some(PebbleValue::List(list))
    }

    fn eval_filter(
        &mut self,
        name: &str,
        base: &Expr,
        args: &[Expr],
    ) -> Option<PebbleValue<'e>> {
        let v = self.eval(base);
        let snake = super::value::camel_to_snake(name);
        match snake.as_str() {
            "length" => match v.as_ref() {
                Some(PebbleValue::Str(s)) => Some(PebbleValue::Int(s.chars().count() as i64)),
                Some(PebbleValue::List(items)) => Some(PebbleValue::Int(items.len() as i64)),
                Some(PebbleValue::Map(items)) => Some(PebbleValue::Int(items.len() as i64)),
                Some(PebbleValue::Null) | None => Some(PebbleValue::Int(0)),
                _ => None,
            },
            "escape" => {
                let mode = match args.first() {
                    Some(a) => self
                        .eval(a)
                        .as_ref()
                        .and_then(|v| match v {
                            PebbleValue::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "html".to_string()),
                    None => "html".to_string(),
                };
                let s = self.display(v.as_ref());
                Some(PebbleValue::Str(self.escape(&mode, &s)))
            }
            "replace" => {
                let s = self.display(v.as_ref());
                let arg0 = args.first().and_then(|a| self.eval(a));
                if let Some(PebbleValue::Map(pairs)) = arg0 {
                    let mut out = s;
                    for (from, to) in &pairs {
                        if let PebbleValue::Str(t) = to {
                            out = out.replace(from.as_str(), t);
                        }
                    }
                    Some(PebbleValue::Str(out))
                } else if args.len() >= 2 {
                    let f_expr = args.first()?;
                    let t_expr = args.get(1)?;
                    let f_val = self.eval(f_expr);
                    let f = self.display(f_val.as_ref());
                    let t_val = self.eval(t_expr);
                    let t = self.display(t_val.as_ref());
                    Some(PebbleValue::Str(s.replace(&f, &t)))
                } else {
                    Some(PebbleValue::Str(s))
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::value::{PebbleObject, PebbleValue, ToPebble};

    struct TestObj {
        site_name: String,
    }

    impl PebbleObject for TestObj {
        fn prop<'p>(&'p self, name: &str) -> Option<PebbleValue<'p>> {
            match name {
                "site_name" => Some(PebbleValue::Str(self.site_name.clone())),
                _ => None,
            }
        }

        fn call<'p>(&'p self, name: &str, _args: &[PebbleValue<'p>]) -> Option<PebbleValue<'p>> {
            match name {
                "upper" => Some(PebbleValue::Str(self.site_name.to_uppercase())),
                "get_site_name" | "site_name" => Some(PebbleValue::Str(self.site_name.clone())),
                _ => None,
            }
        }
    }

    impl ToPebble for TestObj {
        fn to_pebble(&self) -> Option<PebbleValue<'_>> {
            Some(PebbleValue::Obj(self))
        }
    }

    fn render_with<K: ToString>(src: &str, store: HashMap<K, Box<dyn TemplateValue>>) -> String {
        let engine = Engine::new(PathBuf::from("/nonexistent-root"));
        engine.evaluate_source(src, &store)
    }

    fn str_box(s: &str) -> Box<dyn TemplateValue> {
        Box::new(s.to_string())
    }

    fn empty_store() -> HashMap<&'static str, Box<dyn TemplateValue>> {
        HashMap::new()
    }

    fn i32_box(n: i32) -> Box<dyn TemplateValue> {
        Box::new(n)
    }

    fn vec_box(v: Vec<i32>) -> Box<dyn TemplateValue> {
        Box::new(v)
    }

    #[test]
    fn undefined_renders_empty_strict_variables_off() {
        assert_eq!(render_with("{{ missing }}", empty_store()), "");
        assert_eq!(render_with("{{ a.b.c }}", empty_store()), "");
    }

    #[test]
    fn variables_and_literals() {
        let mut store = empty_store();
        store.insert("name", str_box("Havana"));
        assert_eq!(
            render_with("Hello {{ name }} ({{ 1 + 2 }})", store),
            "Hello Havana (3)"
        );
    }

    #[test]
    fn if_elseif_else() {
        let mut store = empty_store();
        store.insert("n", i32_box(1));
        assert_eq!(
            render_with(
                "{% if n == 1 %}one{% elseif n == 2 %}two{% else %}many{% endif %}",
                store
            ),
            "one"
        );
    }

    #[test]
    fn for_set_and_range() {
        let out = render_with(
            "{% set n = 3 %}{% for i in [1..n] %}{{ i }}{% endfor %}",
            empty_store(),
        );
        assert_eq!(out, "123");
    }

    #[test]
    fn escape_filters() {
        let mut store = empty_store();
        store.insert("html", str_box("<b>&</b>"));
        store.insert("js", str_box("a'b"));
        assert_eq!(
            render_with("{{ html }}|{{ html | escape }}|{{ js | escape('js') }}", store),
            "<b>&</b>|&lt;b&gt;&amp;&lt;/b&gt;|a\\'b"
        );
    }

    #[test]
    fn length_filter() {
        let mut store = empty_store();
        store.insert("s", str_box("abc"));
        store.insert("list", vec_box(vec![1, 2, 3]));
        assert_eq!(
            render_with("{{ s | length }}-{{ list | length }}", store),
            "3-3"
        );
    }

    #[test]
    fn is_present_test() {
        let out = render_with(
            "{% set a = 1 %}{% if \"a\" is present %}yes{% endif %}{% if \"b\" is not present %}no{% endif %}",
            empty_store(),
        );
        assert_eq!(out, "yesno");
    }

    #[test]
    fn ternary() {
        let out = render_with(
            "{{ (\"x\" is present) ? x : \"default\" }}",
            empty_store(),
        );
        assert_eq!(out, "default");
    }

    #[test]
    fn equals_and_comparisons() {
        let mut store = empty_store();
        store.insert("list", vec_box(vec![1]));
        assert_eq!(
            render_with(
                "{% if list.size() equals 1 %}eq{% endif %}{% if 2 != 3 %}ne{% endif %}{% if 1 < 2 %}lt{% endif %}{% if 3 not equals 2 %}nq{% endif %}",
                store,
            ),
            "eqneltnq"
        );
    }

    #[test]
    fn entry_set_iteration() {
        let mut store = empty_store();
        let mut map = std::collections::BTreeMap::new();
        map.insert("a".to_string(), 1_i32);
        map.insert("b".to_string(), 2_i32);
        store.insert("m", Box::new(map));
        assert_eq!(
            render_with(
                "{% for e in m.entrySet() %}{{ e.getKey() }}={{ e.getValue() }};{% endfor %}",
                store
            ),
            "a=1;b=2;"
        );
    }


    #[test]
    fn object_name_mapping() {
        let obj = TestObj {
            site_name: "havana".to_string(),
        };
        let mut store: HashMap<&str, Box<dyn TemplateValue>> = HashMap::new();
        store.insert("site", Box::new(obj));
        assert_eq!(
            render_with("{{ site.siteName }}/{{ site.getSiteName() }}/{{ site.upper() }}", store),
            "havana/havana/HAVANA"
        );
    }

    #[test]
    fn autoescape_blocks() {
        assert_eq!(
            render_with("{% autoescape 'html' %}{{ a }}{% endautoescape %}", {
                let mut m: HashMap<&'static str, Box<dyn TemplateValue>> = HashMap::new();
                m.insert("a", str_box("<x>"));
                m
            }),
            "&lt;x&gt;"
        );
        assert_eq!(
            render_with("{% autoescape 'js' %}{{ a }}{% endautoescape %}", {
                let mut m: HashMap<&'static str, Box<dyn TemplateValue>> = HashMap::new();
                m.insert("a", str_box("a'b"));
                m
            }),
            "a\\'b"
        );
    }

    #[test]
    fn include_with_relative_and_root_paths() {
        let dir = std::env::temp_dir().join(format!("pebble_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("base.tpl"), "B:{{ x }}").unwrap();
        std::fs::write(dir.join("other.tpl"), "O").unwrap();
        std::fs::write(
            dir.join("sub").join("child.tpl"),
            "C:{% include \"../base.tpl\" %}{% include \"other.tpl\" %}",
        )
        .unwrap();
        let engine = Engine::new(dir.clone());
        let mut store: HashMap<String, Box<dyn TemplateValue>> = HashMap::new();
        store.insert("x".to_string(), str_box("1"));
        assert_eq!(engine.evaluate("base.tpl", &store), "B:1");
        assert_eq!(engine.evaluate("sub/child.tpl", &store), "C:B:1O");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn smoke_multi_template_tree() {
        let root = std::env::temp_dir().join(format!("pebble_smoke_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("base")).unwrap();
        std::fs::create_dir_all(root.join("housekeeping")).unwrap();
        std::fs::create_dir_all(root.join("account")).unwrap();

        std::fs::write(root.join("layout.tpl"), "<html><body>{% include \"base/header.tpl\" %}</body></html>")
            .unwrap();
        std::fs::write(root.join("base").join("header.tpl"), "header-{{ version }}")
            .unwrap();
        std::fs::write(
            root.join("housekeeping").join("articles.tpl"),
            "{% include \"../layout.tpl\" %}articles{% for i in [1..3] %}{{ i }}{% endfor %}",
        )
        .unwrap();
        std::fs::write(
            root.join("account").join("login.tpl"),
            "{% autoescape 'html' %}login{{ user }}{% endautoescape %}",
        )
        .unwrap();
        std::fs::write(
            root.join("client_install_shockwave.tpl"),
            "{% set v = 2 %}install{{ v + 1 }}",
        )
        .unwrap();

        let engine = Engine::new(root.clone());
        let mut store: HashMap<String, Box<dyn TemplateValue>> = HashMap::new();
        store.insert("version".to_string(), str_box("1"));
        store.insert("user".to_string(), str_box("<Havana>"));
        for rel in [
            "base/header.tpl",
            "housekeeping/articles.tpl",
            "account/login.tpl",
            "client_install_shockwave.tpl",
        ] {
            let out = engine.evaluate(rel, &store);
            eprintln!("SMOKE {rel}: {} chars, starts [{}]", out.len(), out.chars().take(60).collect::<String>());
            assert!(!out.is_empty(), "empty render for {rel}");
        }
        let header = engine.evaluate("base/header.tpl", &store);
        assert!(header.contains("header-1"), "root-relative include unresolved: {header}");
        let articles = engine.evaluate("housekeeping/articles.tpl", &store);
        assert!(articles.contains("header-1"), "../ relative include unresolved: {articles}");
        let login = engine.evaluate("account/login.tpl", &store);
        assert!(login.contains("&lt;Havana&gt;"), "autoescape failed: {login}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bool_and_null_display() {
        let mut store = empty_store();
        store.insert("b", Box::new(true));
        store.insert("n", Box::new(std::option::Option::<i32>::None));
        assert_eq!(render_with("{{ b }}[{{ n }}]", store), "true[]");
    }
}
