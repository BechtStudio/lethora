//! LaTeX math → Typst math for the common cases (Markdown's `$…$`):
//! fractions, roots, scripts, Greek letters, big operators, `\left`/`\right`,
//! font commands, text, accents, spacing, the usual operators, relations and
//! arrows, matrices and cases. A command it doesn't know stays as text in
//! the formula (`"\foo"`, a Typst string), with a warning.

use crate::typst::string;
use crate::{MAX_DEPTH, WarningKind, Warnings};

/// Commands that are one Typst symbol or name.
const SYMBOLS: &[(&str, &str)] = &[
    // Greek
    ("alpha", "alpha"),
    ("beta", "beta"),
    ("gamma", "gamma"),
    ("delta", "delta"),
    ("epsilon", "epsilon.alt"),
    ("varepsilon", "epsilon"),
    ("zeta", "zeta"),
    ("eta", "eta"),
    ("theta", "theta"),
    ("vartheta", "theta.alt"),
    ("iota", "iota"),
    ("kappa", "kappa"),
    ("lambda", "lambda"),
    ("mu", "mu"),
    ("nu", "nu"),
    ("xi", "xi"),
    ("pi", "pi"),
    ("varpi", "pi.alt"),
    ("rho", "rho"),
    ("varrho", "rho.alt"),
    ("sigma", "sigma"),
    ("varsigma", "sigma.alt"),
    ("tau", "tau"),
    ("upsilon", "upsilon"),
    ("phi", "phi.alt"),
    ("varphi", "phi"),
    ("chi", "chi"),
    ("psi", "psi"),
    ("omega", "omega"),
    ("Gamma", "Gamma"),
    ("Delta", "Delta"),
    ("Theta", "Theta"),
    ("Lambda", "Lambda"),
    ("Xi", "Xi"),
    ("Pi", "Pi"),
    ("Sigma", "Sigma"),
    ("Upsilon", "Upsilon"),
    ("Phi", "Phi"),
    ("Psi", "Psi"),
    ("Omega", "Omega"),
    // Big operators and functions
    ("sum", "sum"),
    ("prod", "product"),
    ("coprod", "product.co"),
    ("int", "integral"),
    ("iint", "integral.double"),
    ("iiint", "integral.triple"),
    ("oint", "integral.cont"),
    ("bigcup", "union.big"),
    ("bigcap", "inter.big"),
    ("lim", "lim"),
    ("limsup", "limsup"),
    ("liminf", "liminf"),
    ("sin", "sin"),
    ("cos", "cos"),
    ("tan", "tan"),
    ("cot", "cot"),
    ("sec", "sec"),
    ("csc", "csc"),
    ("arcsin", "arcsin"),
    ("arccos", "arccos"),
    ("arctan", "arctan"),
    ("sinh", "sinh"),
    ("cosh", "cosh"),
    ("tanh", "tanh"),
    ("log", "log"),
    ("ln", "ln"),
    ("lg", "lg"),
    ("exp", "exp"),
    ("max", "max"),
    ("min", "min"),
    ("sup", "sup"),
    ("inf", "inf"),
    ("det", "det"),
    ("dim", "dim"),
    ("ker", "ker"),
    ("deg", "deg"),
    ("gcd", "gcd"),
    ("arg", "arg"),
    ("Pr", "Pr"),
    ("mod", "mod"),
    ("bmod", "mod"),
    // Operators
    ("pm", "plus.minus"),
    ("mp", "minus.plus"),
    ("times", "times"),
    ("div", "div"),
    ("cdot", "dot.op"),
    ("ast", "ast"),
    ("star", "star"),
    ("circ", "compose"),
    ("bullet", "bullet"),
    ("oplus", "plus.circle"),
    ("otimes", "times.circle"),
    ("cup", "union"),
    ("cap", "inter"),
    ("setminus", "without"),
    ("wedge", "and"),
    ("land", "and"),
    ("vee", "or"),
    ("lor", "or"),
    ("neg", "not"),
    ("lnot", "not"),
    // Relations
    ("leq", "<="),
    ("le", "<="),
    ("geq", ">="),
    ("ge", ">="),
    ("neq", "!="),
    ("ne", "!="),
    ("ll", "<<"),
    ("gg", ">>"),
    ("approx", "approx"),
    ("equiv", "equiv"),
    ("sim", "tilde.op"),
    ("simeq", "tilde.eq"),
    ("cong", "tilde.equiv"),
    ("propto", "prop"),
    ("in", "in"),
    ("notin", "in.not"),
    ("ni", "in.rev"),
    ("subset", "subset"),
    ("subseteq", "subset.eq"),
    ("supset", "supset"),
    ("supseteq", "supset.eq"),
    ("perp", "perp"),
    ("parallel", "parallel"),
    ("mid", "divides"),
    ("forall", "forall"),
    ("exists", "exists"),
    ("nexists", "exists.not"),
    // Arrows
    ("to", "->"),
    ("rightarrow", "->"),
    ("leftarrow", "<-"),
    ("gets", "<-"),
    ("leftrightarrow", "<->"),
    ("Rightarrow", "=>"),
    ("implies", "==>"),
    ("Leftarrow", "arrow.l.double"),
    ("Leftrightarrow", "<=>"),
    ("iff", "<==>"),
    ("mapsto", "|->"),
    ("uparrow", "arrow.t"),
    ("downarrow", "arrow.b"),
    // Misc
    ("infty", "infinity"),
    ("partial", "partial"),
    ("nabla", "nabla"),
    ("emptyset", "emptyset"),
    ("varnothing", "emptyset"),
    ("hbar", "planck.reduce"),
    ("ell", "ell"),
    ("Re", "Re"),
    ("Im", "Im"),
    ("aleph", "aleph"),
    ("angle", "angle"),
    ("prime", "prime"),
    ("degree", "degree"),
    ("ldots", "dots.h"),
    ("dots", "dots.h"),
    ("cdots", "dots.c"),
    ("vdots", "dots.v"),
    ("ddots", "dots.down"),
    ("langle", "angle.l"),
    ("rangle", "angle.r"),
    ("lfloor", "floor.l"),
    ("rfloor", "floor.r"),
    ("lceil", "ceil.l"),
    ("rceil", "ceil.r"),
    ("vert", "|"),
    ("lvert", "|"),
    ("rvert", "|"),
    ("Vert", "||"),
    ("|", "||"),
    // Spacing
    (",", "thin"),
    (":", "med"),
    (">", "med"),
    (";", "thick"),
    (" ", "space"),
    ("quad", "quad"),
    ("qquad", "wide"),
    // Escaped characters
    ("{", "\\{"),
    ("}", "\\}"),
    ("%", "%"),
    ("$", "\\$"),
    ("&", "\\&"),
    ("#", "\\#"),
    ("_", "\\_"),
];

/// Commands of one argument that are a Typst function of it.
const FUNCTIONS: &[(&str, &str)] = &[
    ("mathbf", "bold"),
    ("boldsymbol", "bold"),
    ("bm", "bold"),
    ("mathrm", "upright"),
    ("mathit", "italic"),
    ("mathcal", "cal"),
    ("mathscr", "scr"),
    ("mathbb", "bb"),
    ("mathfrak", "frak"),
    ("mathsf", "sans"),
    ("mathtt", "mono"),
    ("hat", "hat"),
    ("widehat", "hat"),
    ("bar", "macron"),
    ("overline", "overline"),
    ("underline", "underline"),
    ("vec", "arrow"),
    ("dot", "dot"),
    ("ddot", "dot.double"),
    ("tilde", "tilde"),
    ("widetilde", "tilde"),
    ("acute", "acute"),
    ("grave", "grave"),
    ("breve", "breve"),
    ("check", "caron"),
    ("overbrace", "overbrace"),
    ("underbrace", "underbrace"),
    ("abs", "abs"),
    ("norm", "norm"),
];

/// Commands that change nothing Typst needs.
const IGNORED: &[&str] = &[
    "displaystyle",
    "textstyle",
    "scriptstyle",
    "scriptscriptstyle",
    "limits",
    "nolimits",
    "!",
    "nonumber",
    "notag",
    "label",
];

/// Commands whose argument is text (`\text{if }`).
const TEXT: &[&str] = &[
    "text",
    "textrm",
    "textit",
    "textbf",
    "textsf",
    "texttt",
    "mbox",
    "mathnormal",
];

/// Delimiter size commands: the delimiter after them is kept, Typst sizes
/// it.
const SIZES: &[&str] = &[
    "left", "right", "middle", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr",
    "biggl", "biggr", "Biggl", "Biggr",
];

/// A row break in an environment.
const BREAK: &str = "\\";
/// A column separator in an environment.
const AMP: &str = "&";

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `\name` or `\` with one other character.
    Cmd(String),
    Open,
    Close,
    Space,
    Char(char),
}

fn lex(latex: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut chars = latex.chars().peekable();
    while let Some(c) = chars.next() {
        toks.push(match c {
            '\\' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek().filter(|c| c.is_ascii_alphabetic()) {
                    name.push(c);
                    chars.next();
                }
                if name.is_empty() {
                    match chars.next() {
                        Some(c) => name.push(c),
                        None => continue,
                    }
                }
                Tok::Cmd(name)
            }
            '{' => Tok::Open,
            '}' => Tok::Close,
            '%' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            c if c.is_whitespace() => Tok::Space,
            c => Tok::Char(c),
        });
    }
    toks
}

/// Where a sequence ends.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    End,
    /// `}`
    Close,
    /// `\end{…}`
    Env,
    /// `]` (a root's index)
    Bracket,
}

struct Tex<'w> {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
    /// Inside a function's arguments: literal `,` `;` `(` `)` are escaped.
    args: usize,
    warnings: &'w mut Warnings,
}

/// `latex` (without the `$`) as Typst math.
pub(crate) fn to_typst(latex: &str, warnings: &mut Warnings) -> String {
    let mut tex = Tex {
        toks: lex(latex),
        pos: 0,
        depth: 0,
        args: 0,
        warnings,
    };
    let atoms = tex.seq(Stop::End);
    join(&atoms)
}

/// Atoms separated by spaces (Typst would read `ab` as one name).
pub(crate) fn join(atoms: &[String]) -> String {
    atoms.join(" ")
}

/// An argument with its parentheses escaped if they don't pair up (an
/// unpaired one would end or extend the function's arguments).
pub(crate) fn balanced(s: String) -> String {
    let mut depth = 0i32;
    let mut paired = true;
    parens(&s, |_, c| {
        depth += if c == '(' { 1 } else { -1 };
        paired &= depth >= 0;
    });
    if paired && depth == 0 {
        return s;
    }
    let mut at = Vec::new();
    parens(&s, |i, _| at.push(i));
    let mut out = s;
    for i in at.into_iter().rev() {
        out.insert(i, '\\');
    }
    out
}

/// Calls `f` with the byte offset of each parenthesis outside strings and
/// escapes.
fn parens(s: &str, mut f: impl FnMut(usize, char)) {
    let mut quoted = false;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => quoted = !quoted,
            '(' | ')' if !quoted => f(i, c),
            _ => {}
        }
    }
}

/// `s`, or an empty string literal standing in for nothing.
pub(crate) fn or_empty(s: String) -> String {
    if s.is_empty() { "\"\"".to_owned() } else { s }
}

/// A character as Typst math: escaped where Typst would read it as syntax
/// (inside a function's arguments, `args`, also the separators and
/// brackets).
pub(crate) fn math_char(c: char, args: bool) -> String {
    match c {
        '/' | '"' | '#' | '@' | '`' | '$' | '\\' => format!("\\{c}"),
        ',' | ';' | ':' | '[' | ']' | '{' | '}' if args => format!("\\{c}"),
        c => c.to_string(),
    }
}

impl Tex<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(&Tok::Space) {
            self.pos += 1;
        }
    }

    /// Atoms up to `stop` (consumed).
    fn seq(&mut self, stop: Stop) -> Vec<String> {
        let mut atoms: Vec<String> = Vec::new();
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Tok::Close => {
                    self.pos += 1;
                    if stop == Stop::Close {
                        break;
                    }
                }
                Tok::Char(']') if stop == Stop::Bracket => {
                    self.pos += 1;
                    break;
                }
                Tok::Cmd(name) if name == "end" => {
                    self.pos += 1;
                    self.raw_group();
                    if stop == Stop::Env {
                        break;
                    }
                }
                Tok::Char(c @ ('^' | '_')) => {
                    self.pos += 1;
                    let script = self.script();
                    if atoms.last().is_none_or(|a| a == BREAK || a == AMP) {
                        atoms.push("\"\"".to_owned());
                    }
                    let last = atoms.last_mut().unwrap();
                    last.push(c);
                    last.push_str(&script);
                }
                Tok::Char(c) if c.is_ascii_digit() => {
                    // A number is one atom.
                    let mut n = String::new();
                    while let Some(Tok::Char(c)) = self.peek() {
                        let more = matches!(self.toks.get(self.pos + 1), Some(Tok::Char(d)) if d.is_ascii_digit());
                        if c.is_ascii_digit() || (*c == '.' && !n.is_empty() && more) {
                            n.push(*c);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    atoms.push(n);
                }
                _ => {
                    if let Some(a) = self.atom() {
                        atoms.push(a);
                    }
                }
            }
        }
        atoms
    }

    /// One token's worth of math (a command with its arguments, a group, a
    /// character), or `None` for what shows nothing.
    fn atom(&mut self) -> Option<String> {
        let tok = self.next()?;
        if self.depth >= MAX_DEPTH {
            if tok == Tok::Open {
                self.skip_group();
            }
            self.warnings.add_example(WarningKind::Math, "deep nesting");
            return Some("\"\"".to_owned());
        }
        self.depth += 1;
        let atom = match tok {
            Tok::Space | Tok::Close => None,
            Tok::Open => Some(self.group()),
            Tok::Char(c) => self.char(c),
            Tok::Cmd(name) => self.command(&name),
        };
        self.depth -= 1;
        atom
    }

    fn char(&mut self, c: char) -> Option<String> {
        Some(match c {
            '~' => return None,
            '&' => AMP.to_owned(),
            c => math_char(c, self.args > 0),
        })
    }

    /// `{…}` after its `{`: the atoms inside, joined.
    fn group(&mut self) -> String {
        or_empty(join(&self.seq(Stop::Close)))
    }

    /// Skips to the `}` closing the group just opened.
    fn skip_group(&mut self) {
        let mut open = 1;
        while let Some(t) = self.next() {
            match t {
                Tok::Open => open += 1,
                Tok::Close => {
                    open -= 1;
                    if open == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// One argument: a group, a command or a single character.
    fn arg(&mut self) -> String {
        self.args += 1;
        self.skip_spaces();
        let arg = match self.peek() {
            None => None,
            Some(Tok::Char(c)) if c.is_ascii_digit() => {
                let c = *c;
                self.pos += 1;
                Some(c.to_string())
            }
            Some(_) => self.atom(),
        };
        self.args -= 1;
        balanced(arg.unwrap_or_else(|| "\"\"".to_owned()))
    }

    /// A script after `^` or `_`: bare if it is one name or number, else in
    /// parentheses (which Typst doesn't show).
    fn script(&mut self) -> String {
        let a = self.arg();
        if a.chars().all(char::is_alphanumeric) && !a.is_empty() {
            a
        } else {
            format!("({a})")
        }
    }

    /// The text of a group (`\text{…}`), raw.
    fn raw_group(&mut self) -> String {
        self.skip_spaces();
        let mut s = String::new();
        if self.peek() != Some(&Tok::Open) {
            if let Some(Tok::Char(c)) = self.peek() {
                s.push(*c);
                self.pos += 1;
            }
            return s;
        }
        self.pos += 1;
        let mut open = 1;
        while let Some(t) = self.next() {
            match t {
                Tok::Open => open += 1,
                Tok::Close => {
                    open -= 1;
                    if open == 0 {
                        break;
                    }
                }
                Tok::Space => s.push(' '),
                Tok::Char(c) => s.push(c),
                Tok::Cmd(name) if name.len() == 1 => s.push_str(&name),
                Tok::Cmd(name) => {
                    s.push('\\');
                    s.push_str(&name);
                }
            }
        }
        s
    }

    fn command(&mut self, name: &str) -> Option<String> {
        if let Some((_, sym)) = SYMBOLS.iter().find(|(n, _)| *n == name) {
            return Some((*sym).to_owned());
        }
        if let Some((_, f)) = FUNCTIONS.iter().find(|(n, _)| *n == name) {
            let a = self.arg();
            return Some(format!("{f}({a})"));
        }
        if IGNORED.contains(&name) {
            if name == "label" {
                self.raw_group();
            }
            return None;
        }
        if TEXT.contains(&name) {
            return Some(string(&self.raw_group()));
        }
        if SIZES.contains(&name) {
            self.skip_spaces();
            return match self.peek() {
                Some(Tok::Char('.')) => {
                    self.pos += 1;
                    None
                }
                _ => self.atom(),
            };
        }
        Some(match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let (a, b) = (self.arg(), self.arg());
                format!("frac({a}, {b})")
            }
            "binom" | "dbinom" | "tbinom" => {
                let (a, b) = (self.arg(), self.arg());
                format!("binom({a}, {b})")
            }
            "sqrt" => {
                self.skip_spaces();
                if self.peek() == Some(&Tok::Char('[')) {
                    self.pos += 1;
                    self.args += 1;
                    let index = balanced(or_empty(join(&self.seq(Stop::Bracket))));
                    self.args -= 1;
                    let x = self.arg();
                    format!("root({index}, {x})")
                } else {
                    format!("sqrt({})", self.arg())
                }
            }
            "operatorname" => format!("op({})", string(&self.raw_group())),
            "\\" => BREAK.to_owned(),
            "begin" => {
                let env = self.raw_group();
                self.env(&env)
            }
            _ => {
                self.warnings
                    .add_example(WarningKind::Math, format!("\\{name}"));
                string(&format!("\\{name}"))
            }
        })
    }

    /// An environment after its `\begin{name}`, up to its `\end{…}`.
    fn env(&mut self, name: &str) -> String {
        let base = name.trim_end_matches('*');
        if matches!(base, "array" | "alignat" | "subarray") {
            // The column spec or count.
            self.raw_group();
        }
        let grid = matches!(
            base,
            "matrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
                | "smallmatrix"
                | "cases"
                | "array"
        );
        if grid {
            self.args += 1;
        }
        let atoms = self.seq(Stop::Env);
        if grid {
            self.args -= 1;
        }
        if !grid {
            if !matches!(
                base,
                "aligned"
                    | "align"
                    | "gathered"
                    | "gather"
                    | "split"
                    | "multline"
                    | "equation"
                    | "eqnarray"
                    | "alignat"
            ) {
                self.warnings
                    .add_example(WarningKind::Math, format!("\\begin{{{name}}}"));
            }
            return join(&atoms);
        }
        // Rows of cells.
        let mut rows: Vec<Vec<String>> = vec![vec![String::new()]];
        for a in atoms {
            match a.as_str() {
                BREAK => rows.push(vec![String::new()]),
                AMP => rows.last_mut().unwrap().push(String::new()),
                _ => {
                    let cell = rows.last_mut().unwrap().last_mut().unwrap();
                    if !cell.is_empty() {
                        cell.push(' ');
                    }
                    cell.push_str(&a);
                }
            }
        }
        if rows.len() > 1 && rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
            rows.pop();
        }
        let cell = |c: &String| balanced(or_empty(c.clone()));
        if base == "cases" {
            let rows: Vec<String> = rows
                .iter()
                .map(|r| r.iter().map(cell).collect::<Vec<_>>().join(" & "))
                .collect();
            return format!("cases({})", rows.join(", "));
        }
        let delim = match base {
            "pmatrix" => "",
            "bmatrix" => "delim: \"[\", ",
            "Bmatrix" => "delim: \"{\", ",
            "vmatrix" => "delim: \"|\", ",
            "Vmatrix" => "delim: \"||\", ",
            _ => "delim: #none, ",
        };
        let rows: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(cell).collect::<Vec<_>>().join(", "))
            .collect();
        format!("mat({delim}{})", rows.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(s: &str) -> String {
        to_typst(s, &mut Warnings::default())
    }

    /// Whether `math` parses as Typst math without errors.
    fn parses(math: &str) -> bool {
        let root = typst_syntax::parse(&format!("$ {math} $"));
        root.errors_and_warnings().0.is_empty()
    }

    #[test]
    fn common_cases() {
        let cases = [
            (r"\frac{a}{b}", "frac(a, b)"),
            (r"\frac12", "frac(1, 2)"),
            (r"\sqrt{x^2+1}", "sqrt(x^2 + 1)"),
            (r"\sqrt[3]{x}", "root(3, x)"),
            (r"x^{2n}", "x^(2 n)"),
            (r"x_i^2", "x_i^2"),
            (r"x^12", "x^1 2"),
            (r"\alpha\beta + \Gamma", "alpha beta + Gamma"),
            (r"\sum_{i=1}^{n} i", "sum_(i = 1)^n i"),
            (
                r"\int_0^\infty e^{-x}\,dx",
                "integral_0^infinity e^(- x) thin d x",
            ),
            (r"\lim_{x \to 0}", "lim_(x -> 0)"),
            (r"\left( \frac{a}{b} \right)", "( frac(a, b) )"),
            (r"\left. x \right|", "x |"),
            (r"\mathbf{v} \cdot \mathrm{d}", "bold(v) dot.op upright(d)"),
            (r"a \leq b \neq c \in \mathbb{R}", "a <= b != c in bb(R)"),
            (r"\text{if } x > 0", "\"if \" x > 0"),
            (r"\frac{a,b}{(c}", "frac(a \\, b, \\( c)"),
            (r"\frac{n(n+1)}{2}", "frac(n ( n + 1 ), 2)"),
            (r"3.14 / 2", "3.14 \\/ 2"),
            (
                r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
                "mat(a, b; c, d)",
            ),
            (
                r"\begin{cases} 1 & x > 0 \\ 0 & \text{else} \end{cases}",
                "cases(1 & x > 0, 0 & \"else\")",
            ),
            (
                r"\begin{aligned} a &= b \\ &= c \end{aligned}",
                "a & = b \\ & = c",
            ),
            (r"{}^{14}C", "\"\"^14 C"),
        ];
        for (latex, typst) in cases {
            assert_eq!(tex(latex), typst, "{latex}");
            assert!(parses(typst), "{typst}");
        }
    }

    #[test]
    fn unknown_commands_stay_as_text() {
        let mut w = Warnings::default();
        let out = to_typst(r"\foo{x} + \bar", &mut w);
        assert_eq!(out, "\"\\\\foo\" x + macron(\"\")");
        assert!(parses(&out));
        let list = w.into_vec();
        assert_eq!(list[0].kind, WarningKind::Math);
        assert_eq!(list[0].example.as_deref(), Some("\\foo"));
    }

    #[test]
    fn odd_input_still_parses() {
        let odd = [
            r"}}}{{{",
            r"\frac{",
            r"x^",
            r"a_{b_{c_{d}}}",
            r"\sqrt[",
            r"\left",
            r"\begin{pmatrix} 1 & \\ \end{pmatrix}",
            r#"" # @ ` / ~ ; , ( ] [ \"#,
            r"\\ & \& \% \$",
            "\\",
            r"\text{a } b}",
            r"\begin{unknown} x \end{unknown}",
            r"αβ x2 ∑",
        ];
        for latex in odd {
            let out = tex(latex);
            assert!(parses(&out), "{latex} → {out}");
        }
        let deep = "{".repeat(500) + "x" + &"}".repeat(500);
        assert!(parses(&tex(&deep)));
    }
}
