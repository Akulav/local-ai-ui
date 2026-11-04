//! Small, dependency-free helpers for the general tools: an arithmetic expression evaluator
//! (`calculate`) and the local date/time (`current_datetime`).

use anyhow::{anyhow, Result};

/// Evaluates an arithmetic expression: + - * / % ^ (or **), parentheses, unary minus, implicit
/// constants (pi, e, tau) and common functions (sqrt, cbrt, abs, sin/cos/tan, asin/acos/atan, atan2,
/// sinh/cosh/tanh, ln, log/log10, log2, exp, floor, ceil, round, trunc, min, max, pow, hypot, fact).
/// Trigonometry works in radians; `deg(x)` / `rad(x)` convert.
pub fn eval(expr: &str) -> Result<f64> {
    let tokens = lex(expr)?;
    let mut p = Parser { t: &tokens, i: 0, depth: 0 };
    let v = p.expr()?;
    if p.i != tokens.len() {
        return Err(anyhow!("unexpected `{}`", tokens[p.i].show()));
    }
    if !v.is_finite() {
        return Err(anyhow!("result is not a finite number"));
    }
    Ok(v)
}

/// Prints a result without float noise (0.1+0.2 → 0.3) and with integers as integers.
pub fn format_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e16 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.12}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

impl Tok {
    fn show(&self) -> String {
        match self {
            Tok::Num(n) => n.to_string(),
            Tok::Ident(s) => s.clone(),
            Tok::Op(c) => c.to_string(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Comma => ",".into(),
        }
    }
}

fn lex(s: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' | '_' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                    i += 1;
                }
                // exponent: 1e5, 2.5E-3
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
                out.push(Tok::Num(text.parse().map_err(|_| anyhow!("bad number `{text}`"))?));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '%' | '^' | '!' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' | '·' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '−' => {
                out.push(Tok::Op('-'));
                i += 1;
            }
            '(' | '[' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' | ']' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' | ';' => {
                out.push(Tok::Comma);
                i += 1;
            }
            other => return Err(anyhow!("unsupported character `{other}`")),
        }
    }
    if out.is_empty() {
        return Err(anyhow!("empty expression"));
    }
    Ok(out)
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    // expr := term (('+'|'-') term)*
    fn expr(&mut self) -> Result<f64> {
        self.depth += 1;
        if self.depth > 200 {
            return Err(anyhow!("expression is nested too deeply"));
        }
        let mut v = self.term()?;
        loop {
            if self.eat(&Tok::Op('+')) {
                v += self.term()?;
            } else if self.eat(&Tok::Op('-')) {
                v -= self.term()?;
            } else {
                break;
            }
        }
        self.depth -= 1;
        Ok(v)
    }

    // term := unary (('*'|'/'|'%') unary | implicit-multiplication)*
    fn term(&mut self) -> Result<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat(&Tok::Op('*')) {
                v *= self.unary()?;
            } else if self.eat(&Tok::Op('/')) {
                let d = self.unary()?;
                if d == 0.0 {
                    return Err(anyhow!("division by zero"));
                }
                v /= d;
            } else if self.eat(&Tok::Op('%')) {
                let d = self.unary()?;
                if d == 0.0 {
                    return Err(anyhow!("modulo by zero"));
                }
                v %= d;
            } else if matches!(self.peek(), Some(Tok::LParen) | Some(Tok::Ident(_))) {
                // 2(3+4), 2pi
                v *= self.unary()?;
            } else {
                break;
            }
        }
        Ok(v)
    }

    // unary := ('-'|'+') unary | power
    fn unary(&mut self) -> Result<f64> {
        if self.eat(&Tok::Op('-')) {
            return Ok(-self.unary()?);
        }
        if self.eat(&Tok::Op('+')) {
            return self.unary();
        }
        self.power()
    }

    // power := postfix ('^' unary)?   (right-associative, -2^2 = -4)
    fn power(&mut self) -> Result<f64> {
        let base = self.postfix()?;
        if self.eat(&Tok::Op('^')) {
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    // postfix := atom '!'*
    fn postfix(&mut self) -> Result<f64> {
        let mut v = self.atom()?;
        while self.eat(&Tok::Op('!')) {
            v = factorial(v)?;
        }
        Ok(v)
    }

    fn atom(&mut self) -> Result<f64> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Ok(n)
            }
            Some(Tok::LParen) => {
                self.i += 1;
                let v = self.expr()?;
                if !self.eat(&Tok::RParen) {
                    return Err(anyhow!("missing `)`"));
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.i += 1;
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.eat(&Tok::Comma) {
                                continue;
                            }
                            if self.eat(&Tok::RParen) {
                                break;
                            }
                            return Err(anyhow!("expected `,` or `)` in {name}(…)"));
                        }
                    }
                    call(&name, &args)
                } else {
                    match name.as_str() {
                        "pi" => Ok(std::f64::consts::PI),
                        "e" => Ok(std::f64::consts::E),
                        "tau" => Ok(std::f64::consts::TAU),
                        "phi" => Ok((1.0 + 5f64.sqrt()) / 2.0),
                        // allow "sqrt 2"-style calls on a single atom
                        f if is_func(f) => {
                            let a = self.postfix()?;
                            call(f, &[a])
                        }
                        other => Err(anyhow!("unknown name `{other}`")),
                    }
                }
            }
            Some(t) => Err(anyhow!("unexpected `{}`", t.show())),
            None => Err(anyhow!("expression ends unexpectedly")),
        }
    }
}

fn is_func(n: &str) -> bool {
    matches!(
        n,
        "sqrt" | "cbrt" | "abs" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "ln" | "log" | "log10" | "log2" | "exp" | "floor" | "ceil"
            | "round" | "trunc" | "sign" | "deg" | "rad" | "fact"
    )
}

fn factorial(v: f64) -> Result<f64> {
    if v < 0.0 || v != v.trunc() || v > 170.0 {
        return Err(anyhow!("factorial needs an integer between 0 and 170"));
    }
    Ok((1..=v as u64).fold(1.0, |acc, k| acc * k as f64))
}

fn call(name: &str, a: &[f64]) -> Result<f64> {
    let one = || -> Result<f64> {
        if a.len() == 1 {
            Ok(a[0])
        } else {
            Err(anyhow!("{name}() takes 1 argument, got {}", a.len()))
        }
    };
    let two = || -> Result<(f64, f64)> {
        if a.len() == 2 {
            Ok((a[0], a[1]))
        } else {
            Err(anyhow!("{name}() takes 2 arguments, got {}", a.len()))
        }
    };
    Ok(match name {
        "sqrt" => {
            let x = one()?;
            if x < 0.0 {
                return Err(anyhow!("sqrt of a negative number"));
            }
            x.sqrt()
        }
        "cbrt" => one()?.cbrt(),
        "abs" => one()?.abs(),
        "sin" => one()?.sin(),
        "cos" => one()?.cos(),
        "tan" => one()?.tan(),
        "asin" => one()?.asin(),
        "acos" => one()?.acos(),
        "atan" => one()?.atan(),
        "atan2" => {
            let (y, x) = two()?;
            y.atan2(x)
        }
        "sinh" => one()?.sinh(),
        "cosh" => one()?.cosh(),
        "tanh" => one()?.tanh(),
        "ln" => one()?.ln(),
        "log" if a.len() == 2 => a[0].ln() / a[1].ln(), // log(x, base)
        "log" | "log10" => one()?.log10(),
        "log2" => one()?.log2(),
        "exp" => one()?.exp(),
        "floor" => one()?.floor(),
        "ceil" => one()?.ceil(),
        "round" if a.len() == 2 => {
            let p = 10f64.powi(a[1] as i32);
            (a[0] * p).round() / p
        }
        "round" => one()?.round(),
        "trunc" => one()?.trunc(),
        "sign" => one()?.signum(),
        "deg" => one()?.to_degrees(),
        "rad" => one()?.to_radians(),
        "fact" => factorial(one()?)?,
        "pow" => {
            let (x, y) = two()?;
            x.powf(y)
        }
        "hypot" => {
            let (x, y) = two()?;
            x.hypot(y)
        }
        "min" if !a.is_empty() => a.iter().cloned().fold(f64::INFINITY, f64::min),
        "max" if !a.is_empty() => a.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        "avg" | "mean" if !a.is_empty() => a.iter().sum::<f64>() / a.len() as f64,
        "sum" => a.iter().sum(),
        other => return Err(anyhow!("unknown function `{other}`")),
    })
}

/// Local date and time, e.g. ("2026-11-05 14:03:22", "Thursday", "+02:00", "GTB Standard Time").
#[cfg(windows)]
pub fn local_now() -> (String, String, String, String) {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let mut tz = TIME_ZONE_INFORMATION::default();
    let which = unsafe { GetTimeZoneInformation(&mut tz) };
    let wide = if which == 2 { &tz.DaylightName[..] } else { &tz.StandardName[..] };
    let end = wide.iter().position(|c| *c == 0).unwrap_or(wide.len());
    let tz_name = String::from_utf16_lossy(&wide[..end]);
    // offset = local - UTC, from the two clocks (minutes, rounded to 15)
    let mins = |t: &windows::Win32::Foundation::SYSTEMTIME| days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64) * 1440 + t.wHour as i64 * 60 + t.wMinute as i64;
    let off = ((mins(&l) - mins(&u)) as f64 / 15.0).round() as i64 * 15;
    let days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    (
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", l.wYear, l.wMonth, l.wDay, l.wHour, l.wMinute, l.wSecond),
        days[(l.wDayOfWeek as usize) % 7].to_string(),
        format!("{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60),
        tz_name,
    )
}

#[cfg(not(windows))]
pub fn local_now() -> (String, String, String, String) {
    let secs = crate::state::now_ms() / 1000;
    let days = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
    let t = secs.rem_euclid(86400);
    (
        format!("{} {:02}:{:02}:{:02}", crate::chat::ymd(secs), t / 3600, t % 3600 / 60, t % 60),
        days[(secs.div_euclid(86400).rem_euclid(7)) as usize].to_string(),
        "+00:00".into(),
        "UTC".into(),
    )
}

#[cfg(windows)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let c = |s: &str| format_number(eval(s).unwrap());
        assert_eq!(c("1 + 2 * 3"), "7");
        assert_eq!(c("(1 + 2) * 3"), "9");
        assert_eq!(c("2 ^ 3 ^ 2"), "512");
        assert_eq!(c("-2^2"), "-4");
        assert_eq!(c("0.1 + 0.2"), "0.3");
        assert_eq!(c("10 % 4"), "2");
        assert_eq!(c("sqrt(16) + abs(-3)"), "7");
        assert_eq!(c("2pi"), format_number(std::f64::consts::TAU));
        assert_eq!(c("5!"), "120");
        assert_eq!(c("max(1, 9, 4)"), "9");
        assert_eq!(c("log(8, 2)"), "3");
        assert_eq!(c("round(3.14159, 2)"), "3.14");
        assert_eq!(c("1e3 * 2"), "2000");
        assert_eq!(c("1_000 * 3"), "3000");
        assert_eq!(c("2**10"), "1024");
        assert!(eval("1/0").is_err());
        assert!(eval("1 +").is_err());
        assert!(eval("foo(2)").is_err());
        assert!(eval("2 3").is_err());
    }
}
