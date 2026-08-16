//! Parses and evaluates gettext `Plural-Forms` header expressions.
//!
//! A `.mo` catalog's header entry (empty msgid) carries a line like:
//! `Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11) ? 0 : ((n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20)) ? 1 : 2);`
//! The `plural` part is a small C conditional-expression language with one free variable, `n`.
//! This module tokenizes and parses that expression once per catalog (at load time), then
//! evaluates it cheaply for each `ngettext` call.

#[derive(Debug, Clone, PartialEq)]
enum Token {
	Num(i64),
	N,
	Question,
	Colon,
	OrOr,
	AndAnd,
	EqEq,
	Ne,
	Le,
	Ge,
	Lt,
	Gt,
	Plus,
	Minus,
	Star,
	Slash,
	Percent,
	Bang,
	LParen,
	RParen,
}

fn tokenize(s: &str) -> Option<Vec<Token>> {
	let bytes = s.as_bytes();
	let mut i = 0;
	let mut toks = Vec::new();
	while i < bytes.len() {
		let c = bytes[i] as char;
		if c.is_ascii_whitespace() {
			i += 1;
			continue;
		}
		let two = |b: u8| bytes.get(i + 1).copied() == Some(b);
		match c {
			'0'..='9' => {
				let start = i;
				while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
					i += 1;
				}
				toks.push(Token::Num(s[start..i].parse().ok()?));
				continue;
			}
			'n' => toks.push(Token::N),
			'?' => toks.push(Token::Question),
			':' => toks.push(Token::Colon),
			'(' => toks.push(Token::LParen),
			')' => toks.push(Token::RParen),
			'+' => toks.push(Token::Plus),
			'-' => toks.push(Token::Minus),
			'*' => toks.push(Token::Star),
			'/' => toks.push(Token::Slash),
			'%' => toks.push(Token::Percent),
			'!' if two(b'=') => {
				toks.push(Token::Ne);
				i += 1;
			}
			'!' => toks.push(Token::Bang),
			'=' if two(b'=') => {
				toks.push(Token::EqEq);
				i += 1;
			}
			'<' if two(b'=') => {
				toks.push(Token::Le);
				i += 1;
			}
			'<' => toks.push(Token::Lt),
			'>' if two(b'=') => {
				toks.push(Token::Ge);
				i += 1;
			}
			'>' => toks.push(Token::Gt),
			'&' if two(b'&') => {
				toks.push(Token::AndAnd);
				i += 1;
			}
			'|' if two(b'|') => {
				toks.push(Token::OrOr);
				i += 1;
			}
			_ => return None,
		}
		i += 1;
	}
	Some(toks)
}

#[derive(Debug, Clone, Copy)]
enum BinOp {
	Or,
	And,
	Eq,
	Ne,
	Lt,
	Le,
	Gt,
	Ge,
	Add,
	Sub,
	Mul,
	Div,
	Mod,
}

#[derive(Debug, Clone)]
enum Expr {
	N,
	Num(i64),
	Not(Box<Expr>),
	Neg(Box<Expr>),
	Bin(BinOp, Box<Expr>, Box<Expr>),
	Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

struct Parser<'a> {
	toks: &'a [Token],
	pos: usize,
}

impl<'a> Parser<'a> {
	fn peek(&self) -> Option<&Token> {
		self.toks.get(self.pos)
	}

	fn eat(&mut self, t: &Token) -> bool {
		if self.peek() == Some(t) {
			self.pos += 1;
			true
		} else {
			false
		}
	}

	/// Ternary is right-associative and lowest precedence, matching C's `?:`.
	fn ternary(&mut self) -> Option<Expr> {
		let cond = self.or()?;
		if self.eat(&Token::Question) {
			let then_ = self.ternary()?;
			if !self.eat(&Token::Colon) {
				return None;
			}
			let else_ = self.ternary()?;
			Some(Expr::Ternary(Box::new(cond), Box::new(then_), Box::new(else_)))
		} else {
			Some(cond)
		}
	}

	fn or(&mut self) -> Option<Expr> {
		let mut lhs = self.and()?;
		while self.eat(&Token::OrOr) {
			lhs = Expr::Bin(BinOp::Or, Box::new(lhs), Box::new(self.and()?));
		}
		Some(lhs)
	}

	fn and(&mut self) -> Option<Expr> {
		let mut lhs = self.equality()?;
		while self.eat(&Token::AndAnd) {
			lhs = Expr::Bin(BinOp::And, Box::new(lhs), Box::new(self.equality()?));
		}
		Some(lhs)
	}

	fn equality(&mut self) -> Option<Expr> {
		let mut lhs = self.relational()?;
		loop {
			if self.eat(&Token::EqEq) {
				lhs = Expr::Bin(BinOp::Eq, Box::new(lhs), Box::new(self.relational()?));
			} else if self.eat(&Token::Ne) {
				lhs = Expr::Bin(BinOp::Ne, Box::new(lhs), Box::new(self.relational()?));
			} else {
				return Some(lhs);
			}
		}
	}

	fn relational(&mut self) -> Option<Expr> {
		let mut lhs = self.additive()?;
		loop {
			if self.eat(&Token::Lt) {
				lhs = Expr::Bin(BinOp::Lt, Box::new(lhs), Box::new(self.additive()?));
			} else if self.eat(&Token::Le) {
				lhs = Expr::Bin(BinOp::Le, Box::new(lhs), Box::new(self.additive()?));
			} else if self.eat(&Token::Gt) {
				lhs = Expr::Bin(BinOp::Gt, Box::new(lhs), Box::new(self.additive()?));
			} else if self.eat(&Token::Ge) {
				lhs = Expr::Bin(BinOp::Ge, Box::new(lhs), Box::new(self.additive()?));
			} else {
				return Some(lhs);
			}
		}
	}

	fn additive(&mut self) -> Option<Expr> {
		let mut lhs = self.multiplicative()?;
		loop {
			if self.eat(&Token::Plus) {
				lhs = Expr::Bin(BinOp::Add, Box::new(lhs), Box::new(self.multiplicative()?));
			} else if self.eat(&Token::Minus) {
				lhs = Expr::Bin(BinOp::Sub, Box::new(lhs), Box::new(self.multiplicative()?));
			} else {
				return Some(lhs);
			}
		}
	}

	fn multiplicative(&mut self) -> Option<Expr> {
		let mut lhs = self.unary()?;
		loop {
			if self.eat(&Token::Star) {
				lhs = Expr::Bin(BinOp::Mul, Box::new(lhs), Box::new(self.unary()?));
			} else if self.eat(&Token::Slash) {
				lhs = Expr::Bin(BinOp::Div, Box::new(lhs), Box::new(self.unary()?));
			} else if self.eat(&Token::Percent) {
				lhs = Expr::Bin(BinOp::Mod, Box::new(lhs), Box::new(self.unary()?));
			} else {
				return Some(lhs);
			}
		}
	}

	fn unary(&mut self) -> Option<Expr> {
		if self.eat(&Token::Bang) {
			return Some(Expr::Not(Box::new(self.unary()?)));
		}
		if self.eat(&Token::Minus) {
			return Some(Expr::Neg(Box::new(self.unary()?)));
		}
		self.primary()
	}

	fn primary(&mut self) -> Option<Expr> {
		match self.peek()?.clone() {
			Token::Num(v) => {
				self.pos += 1;
				Some(Expr::Num(v))
			}
			Token::N => {
				self.pos += 1;
				Some(Expr::N)
			}
			Token::LParen => {
				self.pos += 1;
				let e = self.ternary()?;
				if !self.eat(&Token::RParen) {
					return None;
				}
				Some(e)
			}
			_ => None,
		}
	}
}

fn parse_expr(s: &str) -> Option<Expr> {
	let toks = tokenize(s)?;
	let mut parser = Parser { toks: &toks, pos: 0 };
	let expr = parser.ternary()?;
	if parser.pos == parser.toks.len() { Some(expr) } else { None }
}

/// C semantics: comparisons and logical operators yield 0/1. Not truly short-circuiting (both
/// sides of `&&`/`||` are always evaluated), which is fine since plural expressions are pure
/// arithmetic with no side effects to skip.
fn eval(expr: &Expr, n: i64) -> i64 {
	match expr {
		Expr::Num(v) => *v,
		Expr::N => n,
		Expr::Not(e) => i64::from(eval(e, n) == 0),
		Expr::Neg(e) => -eval(e, n),
		Expr::Ternary(c, t, f) => {
			if eval(c, n) != 0 {
				eval(t, n)
			} else {
				eval(f, n)
			}
		}
		Expr::Bin(op, l, r) => {
			let lv = eval(l, n);
			let rv = eval(r, n);
			match op {
				BinOp::Or => i64::from(lv != 0 || rv != 0),
				BinOp::And => i64::from(lv != 0 && rv != 0),
				BinOp::Eq => i64::from(lv == rv),
				BinOp::Ne => i64::from(lv != rv),
				BinOp::Lt => i64::from(lv < rv),
				BinOp::Le => i64::from(lv <= rv),
				BinOp::Gt => i64::from(lv > rv),
				BinOp::Ge => i64::from(lv >= rv),
				BinOp::Add => lv + rv,
				BinOp::Sub => lv - rv,
				BinOp::Mul => lv * rv,
				BinOp::Div => {
					if rv == 0 {
						0
					} else {
						lv / rv
					}
				}
				BinOp::Mod => {
					if rv == 0 {
						0
					} else {
						lv % rv
					}
				}
			}
		}
	}
}

/// A parsed `Plural-Forms` rule: the number of plural forms the language has, and the
/// expression that picks a form index (0-based) for a given count.
pub(crate) struct PluralRule {
	nplurals: usize,
	expr: Expr,
}

impl PluralRule {
	/// Parses a `.mo` header's `Plural-Forms: nplurals=N; plural=EXPR;` line, if present.
	/// Returns `None` if the header lacks the line or the expression fails to parse, so callers
	/// can fall back to a simplified rule.
	pub(crate) fn parse_from_header(header: &str) -> Option<Self> {
		let line = header.lines().find(|l| l.trim_start().starts_with("Plural-Forms:"))?;
		let rest = line.trim_start().strip_prefix("Plural-Forms:")?;
		let nplurals_str = rest.split("nplurals=").nth(1)?;
		let nplurals_digits: String = nplurals_str.chars().take_while(char::is_ascii_digit).collect();
		let nplurals: usize = nplurals_digits.parse().ok()?;
		if nplurals == 0 {
			return None;
		}
		let plural_part = rest.split("plural=").nth(1)?;
		let expr_str = plural_part.split(';').next()?;
		let expr = parse_expr(expr_str)?;
		Some(Self { nplurals, expr })
	}

	/// Evaluates the rule for count `n`, returning a form index clamped to `[0, nplurals)`.
	pub(crate) fn index_for(&self, n: u64) -> usize {
		let n = i64::try_from(n).unwrap_or(i64::MAX);
		let idx = eval(&self.expr, n);
		let idx = usize::try_from(idx).unwrap_or(0);
		idx.min(self.nplurals.saturating_sub(1))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn english_style_rule() {
		let rule = PluralRule::parse_from_header("Plural-Forms: nplurals=2; plural=(n != 1);\n").unwrap();
		assert_eq!(rule.index_for(0), 1);
		assert_eq!(rule.index_for(1), 0);
		assert_eq!(rule.index_for(2), 1);
	}

	#[test]
	fn russian_style_three_form_rule() {
		let header = "Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n";
		let rule = PluralRule::parse_from_header(header).unwrap();
		assert_eq!(rule.index_for(1), 0);
		assert_eq!(rule.index_for(21), 0);
		assert_eq!(rule.index_for(2), 1);
		assert_eq!(rule.index_for(22), 1);
		assert_eq!(rule.index_for(5), 2);
		assert_eq!(rule.index_for(11), 2);
		assert_eq!(rule.index_for(25), 2);
	}

	#[test]
	fn arabic_style_six_form_rule() {
		let header = "Plural-Forms: nplurals=6; plural=(n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5);\n";
		let rule = PluralRule::parse_from_header(header).unwrap();
		assert_eq!(rule.index_for(0), 0);
		assert_eq!(rule.index_for(1), 1);
		assert_eq!(rule.index_for(2), 2);
		assert_eq!(rule.index_for(3), 3);
		assert_eq!(rule.index_for(11), 4);
		assert_eq!(rule.index_for(100), 5);
		assert_eq!(rule.index_for(203), 3);
	}

	#[test]
	fn missing_header_returns_none() {
		assert!(PluralRule::parse_from_header("Language: de\n").is_none());
	}

	#[test]
	fn malformed_expression_returns_none() {
		assert!(PluralRule::parse_from_header("Plural-Forms: nplurals=2; plural=(n @ 1);\n").is_none());
	}

	#[test]
	fn index_is_clamped_to_nplurals() {
		// A pathological/handwritten rule that could return an out-of-range index.
		let rule = PluralRule::parse_from_header("Plural-Forms: nplurals=2; plural=5;\n").unwrap();
		assert_eq!(rule.index_for(0), 1);
	}
}
