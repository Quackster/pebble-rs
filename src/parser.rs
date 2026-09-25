// Port of the parser of the external `io.pebbletemplates:pebble` template
// engine (Pebble 3.1.5), implemented inline since the library is not in the
// repo. Parses the lexer tokens into the statement tree, and each `{{ ... }}`
// / tag payload into the expression tree (Pebble grammar, scoped to the
// constructs the `default` template set uses).
use super::lexer::Token;

#[derive(Debug, Clone)]
pub enum Lit {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(Lit),
    Var(String),
    Not(Box<Expr>),
    Binary {
        op: &'static str,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Test {
        name: String,
        negated: bool,
        expr: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    ListLit(Vec<Expr>),
    DictLit(Vec<(String, Expr)>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Member {
        base: Box<Expr>,
        name: String,
    },
    Call {
        base: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    Filter {
        base: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Text(String),
    Expr(Box<Expr>),
    If(Vec<(Box<Expr>, Vec<Stmt>)>, Vec<Stmt>),
    For {
        target: String,
        iterable: Box<Expr>,
        body: Vec<Stmt>,
    },
    Set {
        name: String,
        value: Box<Expr>,
    },
    Include(Box<Expr>),
    Autoescape(Box<Expr>, Vec<Stmt>),
}

#[derive(Debug, Clone)]
enum Tok {
    Ident(String),
    Str(String),
    Int(i64),
    Float(f64),
    Op(&'static str),
    Punct(char),
}

fn op_token(s: &str) -> &'static str {
    match s {
        "==" => "==",
        "!=" => "!=",
        "<=" => "<=",
        ">=" => ">=",
        ".." => "..",
        _ => match s.chars().next() {
            Some('<') => "<",
            Some('>') => ">",
            Some('+') => "+",
            Some('-') => "-",
            Some('*') => "*",
            Some('/') => "/",
            Some('%') => "%",
            Some('=') => "=",
            _ => "",
        },
    }
}

fn tokenize_expr(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        let e = chars[i + 1];
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '\\' => '\\',
                            '\'' => '\'',
                            '"' => '"',
                            other => other,
                        });
                        i += 2;
                    } else {
                        s.push(chars[i]);
                        i += 1;
                    }
                }
                i += 1;
                out.push(Tok::Str(s));
            }
            _ if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                // A `.` starts a fraction only when followed by a digit
                // (`1..3` is a range, not a number).
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let num: String = chars[start..i].iter().collect();
                if num.contains('.') {
                    out.push(Tok::Float(num.parse().unwrap_or(0.0)));
                } else {
                    out.push(Tok::Int(num.parse().unwrap_or(0)));
                }
            }
            _ if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_')
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if let Some(tok) = two.get(0..2) {
                    match tok {
                        "==" | "!=" | "<=" | ">=" | ".." => {
                            out.push(Tok::Op(op_token(tok)));
                            i += 2;
                            continue;
                        }
                        _ => {}
                    }
                }
                match c {
                    '<' | '>' | '+' | '-' | '*' | '/' | '%' | '=' => {
                        out.push(Tok::Op(op_token(&c.to_string())));
                        i += 1;
                    }
                    '.' | '(' | ')' | '[' | ']' | '|' | '?' | ':' | '{' | '}' | ',' => {
                        out.push(Tok::Punct(c));
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

struct P {
    toks: Vec<Tok>,
    i: usize,
}

impl P {
    fn new(src: &str) -> Self {
        Self {
            toks: tokenize_expr(src),
            i: 0,
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn peek2(&self) -> Option<&Tok> {
        self.toks.get(self.i + 1)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        self.i += 1;
        t
    }

    fn is_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == name)
    }

    fn is_punct(&self, c: char) -> bool {
        matches!(self.peek(), Some(Tok::Punct(p)) if *p == c)
    }

    fn parse_ternary(&mut self) -> Expr {
        let cond = self.parse_or();
        if self.is_punct('?') {
            self.next();
            let then = self.parse_ternary();
            if !self.is_punct(':') {
                return Expr::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    els: Box::new(Expr::Lit(Lit::Null)),
                };
            }
            self.next();
            let els = self.parse_ternary();
            Expr::Ternary {
                cond: Box::new(cond),
                then: Box::new(then),
                els: Box::new(els),
            }
        } else {
            cond
        }
    }

    fn parse_or(&mut self) -> Expr {
        let mut lhs = self.parse_and();
        while self.is_ident("or") {
            self.next();
            let rhs = self.parse_and();
            lhs = Expr::Binary {
                op: "or",
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_and(&mut self) -> Expr {
        let mut lhs = self.parse_not();
        while self.is_ident("and") {
            self.next();
            let rhs = self.parse_not();
            lhs = Expr::Binary {
                op: "and",
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_not(&mut self) -> Expr {
        // `not equals` is a comparison operator, not a unary `not`.
        if self.is_ident("not") && matches!(self.peek2(), Some(Tok::Ident(s)) if s == "equals") {
            return self.parse_cmp();
        }
        if self.is_ident("not") {
            self.next();
            return Expr::Not(Box::new(self.parse_not()));
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Expr {
        let mut lhs = self.parse_add();
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) => match *o {
                    "==" | "!=" | "<" | ">" | "<=" | ">=" => Some(*o),
                    _ => None,
                },
                Some(Tok::Ident(s)) if s == "equals" => Some("equals"),
                Some(Tok::Ident(s))
                    if s == "not"
                        && matches!(
                            self.peek2(),
                            Some(Tok::Ident(s2)) if s2 == "equals"
                        ) =>
                {
                    Some("not_equals")
                }
                _ => None,
            };
            let Some(op) = op else {
                return lhs;
            };
            self.next();
            if op == "not_equals" {
                self.next();
            }
            let rhs = self.parse_add();
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_add(&mut self) -> Expr {
        let mut lhs = self.parse_mul();
        loop {
            let Some(Tok::Op(o)) = self.peek().cloned() else {
                return lhs;
            };
            if !matches!(o, "+" | "-") {
                return lhs;
            }
            self.next();
            let rhs = self.parse_mul();
            lhs = Expr::Binary {
                op: o,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_mul(&mut self) -> Expr {
        let mut lhs = self.parse_unary();
        loop {
            let Some(Tok::Op(o)) = self.peek().cloned() else {
                return lhs;
            };
            if !matches!(o, "*" | "/" | "%") {
                return lhs;
            }
            self.next();
            let rhs = self.parse_unary();
            lhs = Expr::Binary {
                op: o,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_unary(&mut self) -> Expr {
        if matches!(self.peek(), Some(Tok::Op("-"))) {
            self.next();
            return Expr::Binary {
                op: "-",
                lhs: Box::new(Expr::Lit(Lit::Int(-1))),
                rhs: Box::new(self.parse_unary()),
            };
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut e = self.parse_chain();
        if self.is_ident("is") {
            self.next();
            let negated = self.is_ident("not");
            if negated {
                self.next();
            }
            let name = match self.next() {
                Some(Tok::Ident(s)) => s,
                _ => String::new(),
            };
            e = Expr::Test {
                name,
                negated,
                expr: Box::new(e),
            };
        }
        e
    }

    fn parse_chain(&mut self) -> Expr {
        let mut e = self.parse_primary();
        loop {
            if self.is_punct('.') {
                self.next();
                let Some(Tok::Ident(name)) = self.next() else {
                    return e;
                };
                if self.is_punct('(') {
                    self.next();
                    let args = self.parse_args();
                    e = Expr::Call {
                        base: Box::new(e),
                        name,
                        args,
                    };
                } else {
                    e = Expr::Member {
                        base: Box::new(e),
                        name,
                    };
                }
            } else if self.is_punct('[') {
                self.next();
                let index = self.parse_ternary();
                if self.is_punct(']') {
                    self.next();
                }
                e = Expr::Index {
                    base: Box::new(e),
                    index: Box::new(index),
                };
            } else if self.is_punct('|') {
                self.next();
                let Some(Tok::Ident(name)) = self.next() else {
                    return e;
                };
                let mut args = Vec::new();
                if self.is_punct('(') {
                    self.next();
                    args = self.parse_args();
                }
                e = Expr::Filter {
                    base: Box::new(e),
                    name,
                    args,
                };
            } else {
                return e;
            }
        }
    }

    fn parse_args(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        if self.is_punct(')') {
            self.next();
            return args;
        }
        loop {
            args.push(self.parse_ternary());
            if self.is_punct(',') {
                self.next();
                continue;
            }
            if self.is_punct(')') {
                self.next();
                return args;
            }
            return args;
        }
    }

    fn parse_primary(&mut self) -> Expr {
        let Some(tok) = self.next() else {
            return Expr::Lit(Lit::Null);
        };
        match tok {
            Tok::Str(s) => Expr::Lit(Lit::Str(s)),
            Tok::Int(n) => {
                // `[1..n]` range literal.
                if matches!(self.peek(), Some(Tok::Op(".."))) {
                    self.next();
                    let end = self.parse_ternary();
                    if self.is_punct(']') {
                        self.next();
                    }
                    return Expr::Range {
                        start: Box::new(Expr::Lit(Lit::Int(n))),
                        end: Box::new(end),
                    };
                }
                Expr::Lit(Lit::Int(n))
            }
            Tok::Float(f) => Expr::Lit(Lit::Float(f)),
            Tok::Ident(s) => match s.as_str() {
                "true" => Expr::Lit(Lit::Bool(true)),
                "false" => Expr::Lit(Lit::Bool(false)),
                "null" => Expr::Lit(Lit::Null),
                _ => Expr::Var(s),
            },
            Tok::Punct('(') => {
                let e = self.parse_ternary();
                if self.is_punct(')') {
                    self.next();
                }
                e
            }
            Tok::Punct('[') => {
                let mut items = Vec::new();
                if self.is_punct(']') {
                    self.next();
                    return Expr::ListLit(items);
                }
                loop {
                    items.push(self.parse_ternary());
                    if self.is_punct(',') {
                        self.next();
                        if self.is_punct(']') {
                            self.next();
                            return Expr::ListLit(items);
                        }
                        continue;
                    }
                    if self.is_punct(']') {
                        self.next();
                    }
                    // `[1..n]` is a range literal, not a one-element list.
                    if items.len() == 1
                        && matches!(items[0], Expr::Range { .. })
                    {
                        return items.pop().unwrap();
                    }
                    return Expr::ListLit(items);
                }
            }
            Tok::Punct('{') => {
                let mut items = Vec::new();
                if self.is_punct('}') {
                    self.next();
                    return Expr::DictLit(items);
                }
                loop {
                    let key = match self.next() {
                        Some(Tok::Ident(s)) => s,
                        Some(Tok::Str(s)) => s,
                        _ => String::new(),
                    };
                    let value = if self.is_punct(':') {
                        self.next();
                        self.parse_ternary()
                    } else {
                        Expr::Var(key.clone())
                    };
                    items.push((key, value));
                    if self.is_punct(',') {
                        self.next();
                        if self.is_punct('}') {
                            self.next();
                            return Expr::DictLit(items);
                        }
                        continue;
                    }
                    if self.is_punct('}') {
                        self.next();
                        return Expr::DictLit(items);
                    }
                    return Expr::DictLit(items);
                }
            }
            _ => Expr::Lit(Lit::Null),
        }
    }
}

pub fn parse_expr(src: &str) -> Expr {
    let mut p = P::new(src);
    p.parse_ternary()
}

struct SP<'a> {
    toks: &'a [Token<'a>],
    i: usize,
}

impl<'a> SP<'a> {
    fn current_tag(&self) -> Option<(&'a str, &'a str)> {
        self.toks.get(self.i).and_then(|t| match t {
            Token::Tag(s) => {
                let trimmed = s.trim();
                let kw = trimmed.split_whitespace().next().unwrap_or("");
                Some((kw, trimmed))
            }
            _ => None,
        })
    }

    fn stmts_until(&mut self, ends: &[&str]) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        while self.i < self.toks.len() {
            if let Some((kw, _)) = self.current_tag() {
                if ends.iter().any(|e| *e == kw) {
                    return stmts;
                }
            }
            self.stmt(&mut stmts);
        }
        stmts
    }

    fn stmt(&mut self, out: &mut Vec<Stmt>) {
        let Some(tok) = self.toks.get(self.i) else {
            return;
        };
        match tok {
            Token::Text(t) => {
                out.push(Stmt::Text((*t).to_string()));
                self.i += 1;
            }
            Token::Expr(e) => {
                out.push(Stmt::Expr(Box::new(parse_expr(e))));
                self.i += 1;
            }
            Token::Tag(t) => {
                let trimmed = t.trim();
                let kw = trimmed.split_whitespace().next().unwrap_or("");
                let rest = trimmed.trim_start_matches(kw);
                match kw {
                    "if" => {
                        let cond = parse_expr(rest);
                        self.i += 1;
                        let mut branches = vec![(Box::new(cond), self.stmts_until(&["else", "elseif", "endif"]))];
                        let mut els: Vec<Stmt> = Vec::new();
                        loop {
                            let Some((kw, full)) = self.current_tag() else {
                                break;
                            };
                            match kw {
                                "elseif" => {
                                    let rest = full.trim_start_matches(kw);
                                    let cond = parse_expr(rest);
                                    self.i += 1;
                                    branches.push((
                                        Box::new(cond),
                                        self.stmts_until(&["else", "elseif", "endif"]),
                                    ));
                                }
                                "else" => {
                                    self.i += 1;
                                    els = self.stmts_until(&["endif"]);
                                    break;
                                }
                                "endif" => {
                                    self.i += 1;
                                    break;
                                }
                                _ => break,
                            }
                        }
                        out.push(Stmt::If(branches, els));
                    }
                    "for" => {
                        // `<target> in <iterable>`
                        let target = match rest.find(" in ") {
                            Some(p) => rest[..p].trim().to_string(),
                            None => rest.trim().to_string(),
                        };
                        let iterable_src = match rest.find(" in ") {
                            Some(p) => &rest[p + 4..],
                            None => "",
                        };
                        self.i += 1;
                        let body = self.stmts_until(&["endfor"]);
                        if let Some((kw, _)) = self.current_tag() {
                            if kw == "endfor" {
                                self.i += 1;
                            }
                        }
                        out.push(Stmt::For {
                            target,
                            iterable: Box::new(if iterable_src.trim().is_empty() {
                                Expr::Lit(Lit::Null)
                            } else {
                                parse_expr(iterable_src)
                            }),
                            body,
                        });
                    }
                    "set" => {
                        let (name, value_src) = match rest.find('=') {
                            Some(p) => (rest[..p].trim().to_string(), &rest[p + 1..]),
                            None => (String::new(), ""),
                        };
                        self.i += 1;
                        out.push(Stmt::Set {
                            name,
                            value: Box::new(parse_expr(value_src)),
                        });
                    }
                    "include" => {
                        self.i += 1;
                        out.push(Stmt::Include(Box::new(parse_expr(rest))));
                    }
                    "autoescape" => {
                        self.i += 1;
                        let body = self.stmts_until(&["endautoescape"]);
                        if let Some((kw, _)) = self.current_tag() {
                            if kw == "endautoescape" {
                                self.i += 1;
                            }
                        }
                        let mode = if rest.trim().is_empty() {
                            Expr::Lit(Lit::Str("html".to_string()))
                        } else {
                            parse_expr(rest)
                        };
                        out.push(Stmt::Autoescape(Box::new(mode), body));
                    }
                    _ => {
                        // Unknown tag — dropped (Pebble would raise; the port
                        // degrades to skipping it).
                        self.i += 1;
                    }
                }
            }
        }
    }
}

pub fn parse_stmts(tokens: &[Token]) -> Vec<Stmt> {
    let mut sp = SP { toks: tokens, i: 0 };
    sp.stmts_until(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn exprs(src: &str) -> Vec<Stmt> {
        parse_stmts(&tokenize(src))
    }

    #[test]
    fn parses_if_elseif_else() {
        let stmts = exprs("{% if a %}A{% elseif b %}B{% else %}C{% endif %}");
        assert!(matches!(stmts.len(), 1));
        if let Stmt::If(branches, els) = &stmts[0] {
            assert_eq!(branches.len(), 2);
            assert!(!els.is_empty());
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn parses_for_and_set() {
        let stmts = exprs("{% set n = 3 %}{% for i in [1..n] %}{{ i }}{% endfor %}x");
        assert_eq!(stmts.len(), 3);
    }

    #[test]
    fn parses_ternary_with_test() {
        let e = parse_expr("(\"x\" is present) ? x : \"\"");
        assert!(matches!(e, Expr::Ternary { .. }));
    }

    #[test]
    fn parses_equals_operator() {
        let e = parse_expr("tagList.size() equals 0");
        assert!(matches!(e, Expr::Binary { op: "equals", .. }));
    }

    #[test]
    fn parses_not_equals_operator() {
        let e = parse_expr("a not equals b");
        assert!(matches!(e, Expr::Binary { op: "not_equals", .. }));
    }

    #[test]
    fn parses_unary_not() {
        let e = parse_expr("not a");
        assert!(matches!(e, Expr::Not(_)));
    }

    #[test]
    fn parses_filter_with_arg() {
        let e = parse_expr("x | escape('js')");
        assert!(matches!(e, Expr::Filter { .. }));
    }
}
