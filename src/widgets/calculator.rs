//! Dependency-free arithmetic for the calculator widget: + - * / % ^ and parentheses.

pub fn eval(input: &str) -> Result<f64, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut p = Parser { c: chars, i: 0 };
    let value = p.expr()?;
    p.skip_ws();
    if p.i != p.c.len() {
        return Err(format!("unexpected '{}'", p.c[p.i]));
    }
    Ok(value)
}

struct Parser {
    c: Vec<char>,
    i: usize,
}

impl Parser {
    fn skip_ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.c.get(self.i).copied()
    }
    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Result<f64, String> {
        let mut left = self.term()?;
        while matches!(self.peek(), Some('+') | Some('-')) {
            let op = self.c[self.i];
            self.i += 1;
            let right = self.term()?;
            left = if op == '+' { left + right } else { left - right };
        }
        Ok(left)
    }
    fn term(&mut self) -> Result<f64, String> {
        let mut left = self.factor()?;
        while matches!(self.peek(), Some('*') | Some('/') | Some('%')) {
            let op = self.c[self.i];
            self.i += 1;
            let right = self.factor()?;
            left = match op {
                '*' => left * right,
                '/' => left / right,
                _ => left % right,
            };
        }
        Ok(left)
    }
    fn factor(&mut self) -> Result<f64, String> {
        let base = self.unary()?;
        if self.eat('^') {
            let exp = self.factor()?; // right associative
            return Ok(base.powf(exp));
        }
        Ok(base)
    }
    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some('-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some('+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }
    fn atom(&mut self) -> Result<f64, String> {
        if self.eat('(') {
            let v = self.expr()?;
            if !self.eat(')') {
                return Err("missing ')'".into());
            }
            return Ok(v);
        }
        let mut number = String::new();
        let mut dots = 0;
        while let Some(ch) = self.c.get(self.i).copied() {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.i += 1;
            } else if ch == '.' {
                dots += 1;
                if dots > 1 {
                    return Err("malformed number".into());
                }
                number.push(ch);
                self.i += 1;
            } else {
                break;
            }
        }
        number.parse::<f64>().map_err(|_| "expected a number".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::eval;

    fn near(input: &str, expected: f64) {
        let got = eval(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert!((got - expected).abs() < 1e-9, "{input} = {got}, expected {expected}");
    }

    #[test]
    fn arithmetic() {
        near("1+2*3", 7.0);
        near("(1+2)*3", 9.0);
        near("-4 + 10 / 2", 1.0);
        near("2^3^2", 512.0); // right associative
        near("10 % 3", 1.0);
        near(" 3.5 * 2 ", 7.0);
    }

    #[test]
    fn rejects_non_math() {
        assert!(eval("hello world").is_err());
        assert!(eval("2 +").is_err());
        assert!(eval("(1+2").is_err());
        assert!(eval("1..2").is_err());
    }
}
