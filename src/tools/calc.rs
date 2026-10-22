//! A small, exact-as-f64 expression evaluator for the built-in calculator tool.
//!
//! Supports `+ - * / % ^` (also `**`), parentheses, unary minus, factorial `!`, the constants
//! `pi`, `e` and `tau`, and common functions (`sqrt`, `sin`, `log`, `round`, `min`, …).
//! Trigonometric functions work in radians; `deg()`/`rad()` convert.

use anyhow::{Result, anyhow, bail};

pub fn evaluate(expr: &str) -> Result<f64> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        bail!("empty expression");
    }
    let mut p = Parser { tokens, pos: 0 };
    let v = p.expr()?;
    if let Some(t) = p.peek() {
        bail!("unexpected '{}'", t.text());
    }
    if v.is_nan() {
        bail!("the result is not a number");
    }
    Ok(v)
}

/// Render a result without float noise: integers plainly, others to 15 significant digits.
pub fn format_number(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "∞" } else { "-∞" }.into();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.*e}", 14, v);
    let parsed: f64 = s.parse().unwrap_or(v);
    let plain = format!("{parsed}");
    if plain.len() <= 24 {
        plain
    } else {
        // Very large or small magnitudes: scientific notation, trailing zeros trimmed.
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{mantissa}e{exp}")
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

impl Tok {
    fn text(&self) -> String {
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

fn tokenize(s: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_')
                {
                    i += 1;
                }
                // Exponent: 1e5, 2.5E-3
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
                let n: f64 = text
                    .parse()
                    .map_err(|_| anyhow!("invalid number '{text}'"))?;
                out.push(Tok::Num(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident(
                    chars[start..i].iter().collect::<String>().to_lowercase(),
                ));
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '%' | '^' | '!' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
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
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            other => bail!("unexpected character '{other}'"),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Result<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat(&Tok::Op('+')) {
                v += self.term()?;
            } else if self.eat(&Tok::Op('-')) {
                v -= self.term()?;
            } else {
                return Ok(v);
            }
        }
    }

    /// term := unary (('*' | '/' | '%') unary)*
    fn term(&mut self) -> Result<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat(&Tok::Op('*')) {
                v *= self.unary()?;
            } else if self.eat(&Tok::Op('/')) {
                let d = self.unary()?;
                if d == 0.0 {
                    bail!("division by zero");
                }
                v /= d;
            } else if self.eat(&Tok::Op('%')) {
                let d = self.unary()?;
                if d == 0.0 {
                    bail!("modulo by zero");
                }
                v %= d;
            } else {
                return Ok(v);
            }
        }
    }

    /// unary := ('-' | '+') unary | power
    fn unary(&mut self) -> Result<f64> {
        if self.eat(&Tok::Op('-')) {
            return Ok(-self.unary()?);
        }
        if self.eat(&Tok::Op('+')) {
            return self.unary();
        }
        self.power()
    }

    /// power := postfix ('^' unary)?   (right-associative, so 2^3^2 = 2^9)
    fn power(&mut self) -> Result<f64> {
        let base = self.postfix()?;
        if self.eat(&Tok::Op('^')) {
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    /// postfix := primary '!'*
    fn postfix(&mut self) -> Result<f64> {
        let mut v = self.primary()?;
        while self.eat(&Tok::Op('!')) {
            v = factorial(v)?;
        }
        Ok(v)
    }

    fn primary(&mut self) -> Result<f64> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(n),
            Some(Tok::LParen) => {
                let v = self.expr()?;
                if !self.eat(&Tok::RParen) {
                    bail!("missing ')'");
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.eat(&Tok::RParen) {
                                break;
                            }
                            if !self.eat(&Tok::Comma) {
                                bail!("expected ',' or ')' in {name}(…)");
                            }
                        }
                    }
                    call(&name, &args)
                } else {
                    constant(&name)
                }
            }
            Some(t) => bail!("unexpected '{}'", t.text()),
            None => bail!("the expression ends too early"),
        }
    }
}

fn constant(name: &str) -> Result<f64> {
    Ok(match name {
        "pi" | "π" => std::f64::consts::PI,
        "e" => std::f64::consts::E,
        "tau" => std::f64::consts::TAU,
        "inf" | "infinity" => f64::INFINITY,
        _ => bail!("unknown name '{name}'"),
    })
}

fn factorial(v: f64) -> Result<f64> {
    if v < 0.0 || v != v.trunc() {
        bail!("factorial needs a non-negative integer");
    }
    if v > 170.0 {
        return Ok(f64::INFINITY);
    }
    Ok((1..=v as u64).fold(1.0, |acc, k| acc * k as f64))
}

fn call(name: &str, args: &[f64]) -> Result<f64> {
    let one = |f: fn(f64) -> f64| -> Result<f64> {
        match args {
            [x] => Ok(f(*x)),
            _ => bail!("{name}() takes 1 argument"),
        }
    };
    let two = |f: fn(f64, f64) -> f64| -> Result<f64> {
        match args {
            [a, b] => Ok(f(*a, *b)),
            _ => bail!("{name}() takes 2 arguments"),
        }
    };
    match name {
        "sqrt" => {
            if args.first().is_some_and(|x| *x < 0.0) {
                bail!("sqrt of a negative number");
            }
            one(f64::sqrt)
        }
        "cbrt" => one(f64::cbrt),
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "atan2" => two(f64::atan2),
        "sinh" => one(f64::sinh),
        "cosh" => one(f64::cosh),
        "tanh" => one(f64::tanh),
        "exp" => one(f64::exp),
        "ln" => one(f64::ln),
        "log2" => one(f64::log2),
        "log10" => one(f64::log10),
        "log" => match args {
            [x] => Ok(x.log10()),
            [x, base] => Ok(x.log(*base)),
            _ => bail!("log() takes 1 or 2 arguments"),
        },
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "trunc" => one(f64::trunc),
        "sign" => one(f64::signum),
        "round" => match args {
            [x] => Ok(x.round()),
            [x, digits] => {
                let f = 10f64.powi(*digits as i32);
                Ok((x * f).round() / f)
            }
            _ => bail!("round() takes 1 or 2 arguments"),
        },
        "pow" => two(f64::powf),
        "hypot" => two(f64::hypot),
        "mod" => two(|a, b| a.rem_euclid(b)),
        "deg" | "degrees" => one(f64::to_degrees),
        "rad" | "radians" => one(f64::to_radians),
        "fact" | "factorial" => match args {
            [x] => factorial(*x),
            _ => bail!("{name}() takes 1 argument"),
        },
        "min" | "max" if args.is_empty() => bail!("{name}() needs at least 1 argument"),
        "min" => Ok(args.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => Ok(args.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "sum" => Ok(args.iter().sum()),
        "avg" | "mean" if args.is_empty() => bail!("{name}() needs at least 1 argument"),
        "avg" | "mean" => Ok(args.iter().sum::<f64>() / args.len() as f64),
        _ => bail!("unknown function '{name}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        evaluate(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("2**10"), 1024.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("10 % 4"), 2.0);
        assert_eq!(ev("7 / 2"), 3.5);
        assert_eq!(ev("5!"), 120.0);
        assert_eq!(ev("1_000_000 * 3"), 3_000_000.0);
        assert_eq!(ev("1.5e3 + 2E-1"), 1500.2);
        assert_eq!(ev("12 × 3 ÷ 4 − 1"), 8.0);
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(ev("sqrt(16) + abs(-3)"), 7.0);
        assert!((ev("sin(pi / 2)") - 1.0).abs() < 1e-12);
        assert_eq!(ev("log(1000)"), 3.0);
        assert_eq!(ev("log(8, 2)"), 3.0);
        assert_eq!(ev("round(2.71828, 2)"), 2.72);
        assert_eq!(ev("max(1, 5, 3) + min(4, 2)"), 7.0);
        assert_eq!(ev("avg(2, 4, 6)"), 4.0);
        assert!((ev("deg(pi)") - 180.0).abs() < 1e-9);
    }

    #[test]
    fn errors() {
        for bad in [
            "", "1 +", "(1", "1 / 0", "foo(2)", "2 $ 3", "sqrt(-1)", "1 2", "(-1)!",
        ] {
            assert!(evaluate(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn formats_results() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(-7.0), "-7");
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(1.0 / 3.0), "0.333333333333333");
        assert_eq!(format_number(2f64.powi(80)), "1.20892581961463e24");
        assert_eq!(format_number(f64::INFINITY), "∞");
    }
}
