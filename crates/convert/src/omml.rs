//! Word equations (OMML, `m:oMath`) → Typst math: runs (single letters
//! italic, upright words, known function names), fractions, scripts,
//! roots, big operators, delimiters (matrices and cases in them),
//! functions, accents, bars, braces, limits, matrices, equation arrays,
//! boxes and phantoms. An element it doesn't know keeps its text (a Typst
//! string) with a [`WarningKind::Math`] warning. Escaping and argument
//! handling are [`crate::latex`]'s.

use roxmltree::Node;

use crate::docx::{M, MC, W, alternate, attr, child, is};
use crate::latex::{balanced, join, math_char, or_empty};
use crate::typst::string;
use crate::{MAX_DEPTH, WarningKind, Warnings};

/// Typst's predefined operators (`sin`, `lim`): written as they are.
const FUNCTIONS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "csch", "ctg", "deg",
    "det", "dim", "exp", "gcd", "lcm", "hom", "id", "im", "inf", "ker", "lg", "lim", "liminf",
    "limsup", "ln", "log", "max", "min", "mod", "Pr", "sec", "sech", "sin", "sinc", "sinh", "sup",
    "tan", "tanh", "tg",
];

/// Operators whose limits go below and above.
const LIMITS: &[&str] = &[
    "lim", "liminf", "limsup", "max", "min", "sup", "inf", "det", "gcd", "lcm", "Pr",
];

/// Big operators (`m:nary`'s character).
const NARY: &[(char, &str)] = &[
    ('∑', "sum"),
    ('∏', "product"),
    ('∐', "product.co"),
    ('∫', "integral"),
    ('∬', "integral.double"),
    ('∭', "integral.triple"),
    ('⨌', "integral.quad"),
    ('∮', "integral.cont"),
    ('∯', "integral.surf"),
    ('∰', "integral.vol"),
    ('⋃', "union.big"),
    ('⋂', "inter.big"),
    ('⋁', "or.big"),
    ('⋀', "and.big"),
    ('⨁', "plus.circle.big"),
    ('⨂', "times.circle.big"),
    ('⨀', "dot.circle.big"),
];

/// Accents (`m:acc`'s character, combining or spacing).
const ACCENTS: &[(char, &str)] = &[
    ('\u{302}', "hat"),
    ('^', "hat"),
    ('\u{303}', "tilde"),
    ('~', "tilde"),
    ('\u{304}', "macron"),
    ('\u{305}', "overline"),
    ('¯', "overline"),
    ('\u{307}', "dot"),
    ('\u{308}', "dot.double"),
    ('\u{20db}', "dot.triple"),
    ('\u{20d7}', "arrow"),
    ('→', "arrow"),
    ('\u{20d6}', "arrow.l"),
    ('\u{301}', "acute"),
    ('\u{300}', "grave"),
    ('\u{306}', "breve"),
    ('\u{30c}', "caron"),
    ('\u{30a}', "circle"),
    ('\u{20d1}', "harpoon"),
];

/// Braces and brackets over or under (`m:groupChr`).
const GROUPS: &[(char, &str)] = &[
    ('⏟', "underbrace"),
    ('⏞', "overbrace"),
    ('⎵', "underbracket"),
    ('⎴', "overbracket"),
    ('⏝', "underparen"),
    ('⏜', "overparen"),
];

/// Delimiter pairs Typst matches by itself.
const PAIRS: &[(&str, &str)] = &[
    ("(", ")"),
    ("[", "]"),
    ("{", "}"),
    ("⟨", "⟩"),
    ("|", "|"),
    ("‖", "‖"),
    ("⌊", "⌋"),
    ("⌈", "⌉"),
    ("⟦", "⟧"),
];

struct Omml<'w> {
    depth: usize,
    /// Inside a function's arguments: separators and brackets escaped.
    args: usize,
    /// Inside an equation array: `&` is an alignment point.
    align: usize,
    /// Inside a function name: letters are one word.
    fname: usize,
    warnings: &'w mut Warnings,
}

/// The formula `math` (an `m:oMath`) as Typst math.
pub(crate) fn to_typst(math: Node, warnings: &mut Warnings) -> String {
    let mut o = Omml {
        depth: 0,
        args: 0,
        align: 0,
        fname: 0,
        warnings,
    };
    join(&o.seq(Some(math)))
}

/// The element's `m:val`-style property `name` in its properties `pr`.
fn prop<'a>(pr: Option<Node<'a, '_>>, name: &str) -> Option<&'a str> {
    let p = child(pr?, M, name)?;
    attr(p, M, "val").or(attr(p, W, "val")).or(Some(""))
}

/// An on/off property, `default` if absent.
fn on(pr: Option<Node>, name: &str, default: bool) -> bool {
    match prop(pr, name) {
        None => default,
        Some(v) => !matches!(v, "0" | "false" | "off"),
    }
}

/// The first character of a property (`m:chr`), `default` if absent; `None`
/// if it is set empty.
fn chr(pr: Option<Node>, default: char) -> Option<char> {
    match prop(pr, "chr") {
        None => Some(default),
        Some(v) => v.chars().next(),
    }
}

/// The function for a `m:groupChr` with properties `pr` (`underbrace`).
fn group(pr: Option<Node>) -> Option<&'static str> {
    let c = chr(pr, '⏟');
    GROUPS.iter().find(|(g, _)| Some(*g) == c).map(|(_, f)| *f)
}

/// Whether `s` is one name or number: a script needs no parentheses.
fn bare(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_alphanumeric)
}

/// `^…`/`_…` content: bare if one name or number, else in parentheses
/// (which Typst doesn't show).
fn script(s: String) -> String {
    if bare(&s) {
        s
    } else {
        format!("({})", balanced(or_empty(s)))
    }
}

/// Whether atoms are one atom without scripts of its own (a base `x^2` can
/// take).
fn simple(atoms: &[String]) -> bool {
    let [a] = atoms else {
        return false;
    };
    let mut depth = 0i32;
    let mut quoted = false;
    let mut chars = a.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            '^' | '_' | ' ' if !quoted && depth == 0 => return false,
            _ => {}
        }
    }
    true
}

impl Omml<'_> {
    /// The atoms of `n`'s children.
    fn seq(&mut self, n: Option<Node>) -> Vec<String> {
        let mut atoms = Vec::new();
        for c in n.into_iter().flat_map(|n| n.children()) {
            atoms.extend(self.element(c));
        }
        atoms
    }

    /// The child `name` of `n` as one argument of a function.
    fn arg(&mut self, n: Node, name: &str) -> String {
        self.args += 1;
        let atoms = self.seq(child(n, M, name));
        self.args -= 1;
        balanced(or_empty(join(&atoms)))
    }

    /// The child `name` of `n` as a script (`^…`).
    fn script_of(&mut self, n: Node, name: &str) -> Option<String> {
        let atoms = self.seq(child(n, M, name));
        (!atoms.is_empty()).then(|| script(join(&atoms)))
    }

    fn unknown(&mut self, n: Node) -> Vec<String> {
        self.warnings
            .add_example(WarningKind::Math, format!("m:{}", n.tag_name().name()));
        let text: String = n
            .descendants()
            .filter(|d| is(*d, M, "t"))
            .filter_map(|d| d.text())
            .collect();
        match text.trim() {
            "" => Vec::new(),
            t => vec![string(t)],
        }
    }

    fn element(&mut self, n: Node) -> Vec<String> {
        if !n.is_element() {
            return Vec::new();
        }
        if self.depth >= MAX_DEPTH {
            self.warnings.add_example(WarningKind::Math, "deep nesting");
            return vec!["\"\"".to_owned()];
        }
        self.depth += 1;
        let atoms = self.structure(n);
        self.depth -= 1;
        atoms
    }

    fn structure(&mut self, n: Node) -> Vec<String> {
        if is(n, MC, "AlternateContent") {
            return self.seq(alternate(n));
        }
        if n.tag_name().namespace().is_some_and(|u| W.contains(&u)) {
            return match n.tag_name().name() {
                "ins" | "moveTo" | "smartTag" | "customXml" | "sdtContent" => self.seq(Some(n)),
                "sdt" => self.seq(child(n, W, "sdtContent")),
                "r" => {
                    let t: String = n
                        .children()
                        .filter(|c| is(*c, W, "t"))
                        .filter_map(|c| c.text())
                        .collect();
                    self.text(&t, true, false)
                }
                _ => Vec::new(),
            };
        }
        if !n.tag_name().namespace().is_some_and(|u| M.contains(&u)) {
            return Vec::new();
        }
        let name = n.tag_name().name();
        let pr = child(n, M, &format!("{name}Pr"));
        let one = |s: String| vec![s];
        match name {
            "r" => self.run(n),
            "e" | "num" | "den" | "sub" | "sup" | "deg" | "lim" | "fName" | "oMath" | "box"
            | "borderBox" => match name {
                "box" | "borderBox" => self.seq(child(n, M, "e")),
                _ => self.seq(Some(n)),
            },
            "f" => one(match prop(pr, "type") {
                Some("lin") | Some("skw") => {
                    let num = self.seq(child(n, M, "num"));
                    let den = self.seq(child(n, M, "den"));
                    format!("{} \\/ {}", script(join(&num)), script(join(&den)))
                }
                Some("noBar") => {
                    let (a, b) = (self.arg(n, "num"), self.arg(n, "den"));
                    format!("mat(delim: #none, {a}; {b})")
                }
                _ => {
                    let (a, b) = (self.arg(n, "num"), self.arg(n, "den"));
                    format!("frac({a}, {b})")
                }
            }),
            "sSup" | "sSub" | "sSubSup" => {
                let base = self.seq(child(n, M, "e"));
                let sub = self.script_of(n, "sub");
                let sup = self.script_of(n, "sup");
                one(self.attach(base, sub, sup))
            }
            "sPre" => {
                let base = self.arg(n, "e");
                let mut parts = vec![base];
                if let Some(s) = self.script_of(n, "sub") {
                    parts.push(format!("bl: {s}"));
                }
                if let Some(s) = self.script_of(n, "sup") {
                    parts.push(format!("tl: {s}"));
                }
                one(format!("attach({})", parts.join(", ")))
            }
            "rad" => {
                let x = self.arg(n, "e");
                let deg = self.seq(child(n, M, "deg"));
                one(match on(pr, "degHide", false) || deg.is_empty() {
                    true => format!("sqrt({x})"),
                    false => {
                        let d = self.arg(n, "deg");
                        format!("root({d}, {x})")
                    }
                })
            }
            "nary" => self.nary(n, pr),
            "d" => one(self.delimited(n, pr)),
            "func" => {
                self.fname += 1;
                let mut atoms = self.seq(child(n, M, "fName"));
                self.fname -= 1;
                atoms.extend(self.seq(child(n, M, "e")));
                atoms
            }
            "acc" => {
                let c = chr(pr, '\u{302}');
                match ACCENTS.iter().find(|(a, _)| Some(*a) == c) {
                    Some((_, f)) => one(format!("{f}({})", self.arg(n, "e"))),
                    None => {
                        self.warnings
                            .add_example(WarningKind::Math, format!("m:acc {}", c.unwrap_or(' ')));
                        self.seq(child(n, M, "e"))
                    }
                }
            }
            "bar" => {
                let f = match prop(pr, "pos") {
                    Some("top") => "overline",
                    _ => "underline",
                };
                one(format!("{f}({})", self.arg(n, "e")))
            }
            "groupChr" => match self.group(n, pr) {
                Some(f) => one(format!("{f}({})", self.arg(n, "e"))),
                None => self.seq(child(n, M, "e")),
            },
            "limLow" | "limUpp" => {
                let (mark, side) = match name {
                    "limLow" => ('_', "under"),
                    _ => ('^', "over"),
                };
                // A brace with its annotation: `underbrace(x, n)`.
                let e = child(n, M, "e");
                let only = e.and_then(|e| {
                    let mut kids = e.children().filter(|c| c.is_element());
                    kids.next().filter(|_| kids.next().is_none())
                });
                if let Some(g) = only.filter(|g| is(*g, M, "groupChr")) {
                    let gpr = child(g, M, "groupChrPr");
                    if let Some(f) = group(gpr).filter(|f| f.starts_with(side)) {
                        let body = self.arg(g, "e");
                        let lim = self.arg(n, "lim");
                        return one(format!("{f}({body}, {lim})"));
                    }
                }
                self.args += 1;
                let base = self.seq(e);
                self.args -= 1;
                let lim = self.script_of(n, "lim").unwrap_or_else(|| "\"\"".into());
                let base = match &base[..] {
                    [op] if LIMITS.contains(&op.as_str()) => op.clone(),
                    _ => format!("limits({})", balanced(or_empty(join(&base)))),
                };
                one(format!("{base}{mark}{lim}"))
            }
            "m" => one(self.matrix(n, "delim: #none, ")),
            "eqArr" => {
                self.align += 1;
                let rows: Vec<String> = n
                    .children()
                    .filter(|c| is(*c, M, "e"))
                    .map(|e| join(&self.seq(Some(e))))
                    .collect();
                self.align -= 1;
                one(rows.join(" \\ "))
            }
            "phant" => match on(pr, "show", true) {
                true => self.seq(child(n, M, "e")),
                false => Vec::new(),
            },
            "ctrlPr" | "argPr" => Vec::new(),
            _ if name.ends_with("Pr") => Vec::new(),
            _ => self.unknown(n),
        }
    }

    /// The function for a `m:groupChr` (`underbrace`), if known; one it
    /// doesn't know is warned about.
    fn group(&mut self, n: Node, pr: Option<Node>) -> Option<&'static str> {
        let found = group(pr);
        if found.is_none() {
            let c = chr(pr, '⏟');
            self.warnings.add_example(
                WarningKind::Math,
                format!("m:{} {}", n.tag_name().name(), c.unwrap_or(' ')),
            );
        }
        found
    }

    /// `base` with scripts: `x_i^2`, or `attach(…)` if the base is more than
    /// one atom.
    fn attach(&mut self, base: Vec<String>, sub: Option<String>, sup: Option<String>) -> String {
        if base.is_empty() || simple(&base) {
            let mut s = or_empty(join(&base));
            for (mark, part) in [('_', sub), ('^', sup)] {
                if let Some(p) = part {
                    s.push(mark);
                    s.push_str(&p);
                }
            }
            return s;
        }
        let mut parts = vec![arg_safe(&join(&base))];
        if let Some(s) = sub {
            parts.push(format!("b: {s}"));
        }
        if let Some(s) = sup {
            parts.push(format!("t: {s}"));
        }
        format!("attach({})", parts.join(", "))
    }

    fn nary(&mut self, n: Node, pr: Option<Node>) -> Vec<String> {
        let c = chr(pr, '∫').unwrap_or('∫');
        let mut op = match NARY.iter().find(|(o, _)| *o == c) {
            Some((_, name)) => (*name).to_owned(),
            None => math_char(c, self.args > 0),
        };
        let integral = op.starts_with("integral");
        match prop(pr, "limLoc") {
            Some("undOvr") if integral => op = format!("limits({op})"),
            Some("subSup") if !integral => op = format!("scripts({op})"),
            _ => {}
        }
        let sub = (!on(pr, "subHide", false))
            .then(|| self.script_of(n, "sub"))
            .flatten();
        let sup = (!on(pr, "supHide", false))
            .then(|| self.script_of(n, "sup"))
            .flatten();
        let mut atoms = vec![self.attach(vec![op], sub, sup)];
        atoms.extend(self.seq(child(n, M, "e")));
        atoms
    }

    /// `m:d`: `(…)`, matched pairs as they are, others through `lr(…)`;
    /// a matrix or binomial in parentheses and cases in a brace become
    /// Typst's own.
    fn delimited(&mut self, n: Node, pr: Option<Node>) -> String {
        let side = |name: &str, default: char| match prop(pr, name) {
            None => default.to_string(),
            Some(v) => v.chars().next().map(String::from).unwrap_or_default(),
        };
        let (open, close) = (side("begChr", '('), side("endChr", ')'));
        let sep = side("sepChr", '|');
        let es: Vec<Node> = n.children().filter(|c| is(*c, M, "e")).collect();
        let only = match &es[..] {
            [e] => {
                let mut kids = e
                    .children()
                    .filter(|c| c.is_element() && !is(*c, M, "ctrlPr"));
                kids.next().filter(|_| kids.next().is_none())
            }
            _ => None,
        };
        if let Some(inner) = only {
            if is(inner, M, "m") {
                let delim = match (open.as_str(), close.as_str()) {
                    ("(", ")") => Some(""),
                    ("[", "]") => Some("delim: \"[\", "),
                    ("{", "}") => Some("delim: \"{\", "),
                    ("|", "|") => Some("delim: \"|\", "),
                    ("‖", "‖") => Some("delim: \"||\", "),
                    _ => None,
                };
                if let Some(d) = delim {
                    return self.matrix(inner, d);
                }
            }
            let ipr = child(inner, M, "fPr");
            if is(inner, M, "f")
                && prop(ipr, "type") == Some("noBar")
                && open == "("
                && close == ")"
            {
                let (a, b) = (self.arg(inner, "num"), self.arg(inner, "den"));
                return format!("binom({a}, {b})");
            }
            if is(inner, M, "eqArr") && open == "{" && close.is_empty() {
                self.args += 1;
                self.align += 1;
                let rows: Vec<String> = inner
                    .children()
                    .filter(|c| is(*c, M, "e"))
                    .map(|e| balanced(or_empty(join(&self.seq(Some(e))))))
                    .collect();
                self.align -= 1;
                self.args -= 1;
                return format!("cases({})", rows.join(", "));
            }
        }
        let paired = PAIRS.iter().any(|(o, c)| *o == open && *c == close);
        if !paired {
            self.args += 1;
        }
        let sep = sep.chars().next().map(|c| self.char(c)).unwrap_or_default();
        let mut parts = Vec::new();
        for e in es {
            parts.push(join(&self.seq(Some(e))));
        }
        let inner = parts.join(&format!(" {sep} "));
        if paired {
            return match open.as_str() {
                "(" => format!("({})", balanced(inner)),
                _ => format!("{open} {} {close}", balanced(inner)),
            };
        }
        self.args -= 1;
        let esc = |s: &str| {
            s.chars()
                .map(|c| match c {
                    '(' | ')' => format!("\\{c}"),
                    c => math_char(c, true),
                })
                .collect::<String>()
        };
        let body = [esc(&open), balanced(inner), esc(&close)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        format!("lr({body})")
    }

    /// `m:m` as `mat(…)` with `delim` (`delim: "[", ` or empty).
    fn matrix(&mut self, m: Node, delim: &str) -> String {
        self.args += 1;
        let rows: Vec<String> = m
            .children()
            .filter(|r| is(*r, M, "mr"))
            .map(|r| {
                r.children()
                    .filter(|c| is(*c, M, "e"))
                    .map(|e| balanced(or_empty(join(&self.seq(Some(e))))))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|r| !r.is_empty())
            .collect();
        self.args -= 1;
        format!("mat({delim}{})", rows.join("; "))
    }

    /// `m:r`: its text in its style.
    fn run(&mut self, r: Node) -> Vec<String> {
        let pr = child(r, M, "rPr");
        let text: String = r
            .children()
            .filter(|c| is(*c, M, "t"))
            .filter_map(|c| c.text())
            .collect();
        if text.is_empty() {
            return Vec::new();
        }
        if on(pr, "nor", false) {
            return vec![string(&text)];
        }
        let sty = prop(pr, "sty").unwrap_or("i");
        let wrap = match prop(pr, "scr") {
            Some("double-struck") => Some("bb"),
            Some("script") => Some("cal"),
            Some("fraktur") => Some("frak"),
            Some("sans-serif") => Some("sans"),
            Some("monospace") => Some("mono"),
            _ => None,
        }
        .or(match sty {
            "b" => Some("bold"),
            "bi" => Some("bold"),
            _ => None,
        });
        if wrap.is_some() {
            self.args += 1;
        }
        let atoms = self.text(&text, matches!(sty, "p" | "b"), wrap.is_some());
        let Some(f) = wrap else {
            return atoms;
        };
        self.args -= 1;
        if atoms.is_empty() {
            return atoms;
        }
        let inner = join(&atoms);
        vec![match sty {
            "b" => format!("upright({f}({}))", balanced(inner)),
            _ => format!("{f}({})", balanced(inner)),
        }]
    }

    /// Text as atoms: letters one each (a word if `upright` or in a
    /// function name), numbers whole, the rest as escaped characters.
    fn text(&mut self, text: &str, upright: bool, wrapped: bool) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut atoms = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_alphabetic() {
                let end = chars[i..]
                    .iter()
                    .position(|c| !c.is_alphabetic())
                    .map_or(chars.len(), |n| i + n);
                let word: String = chars[i..end].iter().collect();
                if (upright || self.fname > 0) && !wrapped {
                    atoms.push(if FUNCTIONS.contains(&word.as_str()) {
                        word
                    } else if end - i == 1 {
                        if upright {
                            format!("upright({word})")
                        } else {
                            word
                        }
                    } else if self.fname > 0 {
                        format!("op({})", string(&word))
                    } else {
                        string(&word)
                    });
                } else {
                    atoms.extend(chars[i..end].iter().map(|c| c.to_string()));
                }
                i = end;
            } else if c.is_ascii_digit() {
                let mut end = i;
                while end < chars.len()
                    && (chars[end].is_ascii_digit()
                        || (chars[end] == '.'
                            && chars.get(end + 1).is_some_and(char::is_ascii_digit)))
                {
                    end += 1;
                }
                atoms.push(chars[i..end].iter().collect());
                i = end;
            } else {
                let s = self.char(c);
                if !s.is_empty() {
                    atoms.push(s);
                }
                i += 1;
            }
        }
        atoms
    }

    /// One character that is neither letter nor digit.
    fn char(&self, c: char) -> String {
        match c {
            c if c.is_whitespace() || c.is_control() => String::new(),
            // Invisible operators and zero-width spaces.
            '\u{200b}'..='\u{200d}' | '\u{2060}'..='\u{2064}' | '\u{feff}' => String::new(),
            '&' if self.align > 0 => "&".to_owned(),
            '&' | '^' | '_' | '~' => format!("\\{c}"),
            c => math_char(c, self.args > 0),
        }
    }
}

/// `s` safe as one argument: separators at its top level escaped and
/// parentheses balanced.
fn arg_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    let mut quoted = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                out.extend(chars.next());
                continue;
            }
            '"' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            ',' | ';' | ':' if !quoted && depth <= 0 => out.push('\\'),
            _ => {}
        }
        out.push(c);
    }
    balanced(or_empty(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn omml(inner: &str) -> (String, Vec<crate::Warning>) {
        let xml = format!("<m:oMath {NS}>{inner}</m:oMath>");
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let mut w = Warnings::default();
        let out = to_typst(doc.root_element(), &mut w);
        (out, w.into_vec())
    }

    /// Whether `math` parses as Typst math without errors.
    fn parses(math: &str) -> bool {
        let root = typst_syntax::parse(&format!("$ {math} $"));
        root.errors_and_warnings().0.is_empty()
    }

    fn r(t: &str) -> String {
        format!("<m:r><m:t>{t}</m:t></m:r>")
    }

    fn e(inner: &str) -> String {
        format!("<m:e>{inner}</m:e>")
    }

    #[test]
    fn constructs() {
        let cases = [
            (r("x+1=2.5"), "x + 1 = 2.5"),
            (r("ab"), "a b"),
            (
                format!(
                    "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                    r("a"),
                    r("b+c")
                ),
                "frac(a, b + c)",
            ),
            (
                format!("<m:sSup>{}<m:sup>{}</m:sup></m:sSup>", e(&r("x")), r("2")),
                "x^2",
            ),
            (
                format!(
                    "<m:sSubSup>{}<m:sub>{}</m:sub><m:sup>{}</m:sup></m:sSubSup>",
                    e(&r("x")),
                    r("i,j"),
                    r("n+1")
                ),
                "x_(i , j)^(n + 1)",
            ),
            (
                format!("<m:sSup>{}<m:sup>{}</m:sup></m:sSup>", e(&r("a+b")), r("2")),
                "attach(a + b, t: 2)",
            ),
            (
                format!(
                    "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/>{}</m:rad>",
                    e(&r("x"))
                ),
                "sqrt(x)",
            ),
            (
                format!("<m:rad><m:deg>{}</m:deg>{}</m:rad>", r("3"), e(&r("x"))),
                "root(3, x)",
            ),
            (
                format!(
                    "<m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup>{}</m:nary>",
                    r("i=1"),
                    r("n"),
                    e(&r("i"))
                ),
                "sum_(i = 1)^n i",
            ),
            (
                format!(
                    "<m:nary><m:sub>{}</m:sub><m:sup>{}</m:sup>{}</m:nary>",
                    r("0"),
                    r("∞"),
                    e(&r("f"))
                ),
                "integral_0^(∞) f",
            ),
            (format!("<m:d>{}</m:d>", e(&r("a,b"))), "(a , b)"),
            (
                format!(
                    "<m:d><m:dPr><m:begChr m:val=\"[\"/><m:endChr m:val=\")\"/></m:dPr>{}</m:d>",
                    e(&r("0,1"))
                ),
                "lr(\\[ 0 \\, 1 \\))",
            ),
            (
                format!(
                    "<m:func><m:fName><m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>sin</m:t></m:r></m:fName>{}</m:func>",
                    e(&r("x"))
                ),
                "sin x",
            ),
            (
                format!(
                    "<m:acc><m:accPr><m:chr m:val=\"\u{20d7}\"/></m:accPr>{}</m:acc>",
                    e(&r("v"))
                ),
                "arrow(v)",
            ),
            (format!("<m:acc>{}</m:acc>", e(&r("x"))), "hat(x)"),
            (
                format!(
                    "<m:bar><m:barPr><m:pos m:val=\"top\"/></m:barPr>{}</m:bar>",
                    e(&r("z"))
                ),
                "overline(z)",
            ),
            (
                format!(
                    "<m:func><m:fName><m:limLow>{}<m:lim>{}</m:lim></m:limLow></m:fName>{}</m:func>",
                    e("<m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>lim</m:t></m:r>"),
                    r("x→0"),
                    e(&r("f"))
                ),
                "lim_(x → 0) f",
            ),
            (
                format!(
                    "<m:d><m:dPr><m:begChr m:val=\"[\"/><m:endChr m:val=\"]\"/></m:dPr>{}</m:d>",
                    e(&format!(
                        "<m:m><m:mr>{}{}</m:mr><m:mr>{}{}</m:mr></m:m>",
                        e(&r("a")),
                        e(&r("b")),
                        e(&r("c")),
                        e(&r("d"))
                    ))
                ),
                "mat(delim: \"[\", a, b; c, d)",
            ),
            (
                format!(
                    "<m:d><m:dPr><m:begChr m:val=\"{{\"/><m:endChr m:val=\"\"/></m:dPr>{}</m:d>",
                    e(&format!(
                        "<m:eqArr>{}{}</m:eqArr>",
                        e(&r("1&amp;x&gt;0")),
                        e(&r("0&amp;else"))
                    ))
                ),
                "cases(1 & x > 0, 0 & e l s e)",
            ),
            (
                format!(
                    "<m:eqArr>{}{}</m:eqArr>",
                    e(&r("a&amp;=b")),
                    e(&r("&amp;=c"))
                ),
                "a & = b \\ & = c",
            ),
            (
                format!("<m:groupChr>{}</m:groupChr>", e(&r("a+b"))),
                "underbrace(a + b)",
            ),
            (
                format!(
                    "<m:limUpp>{}<m:lim>{}</m:lim></m:limUpp>",
                    e(&format!(
                        "<m:groupChr><m:groupChrPr><m:chr m:val=\"⏞\"/><m:pos m:val=\"top\"/></m:groupChrPr>{}</m:groupChr>",
                        e(&r("x"))
                    )),
                    r("n")
                ),
                "overbrace(x, n)",
            ),
            (
                "<m:r><m:rPr><m:nor/></m:rPr><m:t>if x</m:t></m:r>".to_owned(),
                "\"if x\"",
            ),
            (
                "<m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>d</m:t></m:r>".to_owned(),
                "upright(d)",
            ),
            (
                "<m:r><m:rPr><m:scr m:val=\"double-struck\"/></m:rPr><m:t>R</m:t></m:r>".to_owned(),
                "bb(R)",
            ),
            (
                format!(
                    "<m:f><m:fPr><m:type m:val=\"lin\"/></m:fPr><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                    r("a+b"),
                    r("2")
                ),
                "(a + b) \\/ 2",
            ),
            (
                format!(
                    "<m:d>{}</m:d>",
                    e(&format!(
                        "<m:f><m:fPr><m:type m:val=\"noBar\"/></m:fPr><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                        r("n"),
                        r("k")
                    ))
                ),
                "binom(n, k)",
            ),
            (
                format!(
                    "<m:sPre><m:sub>{}</m:sub><m:sup>{}</m:sup>{}</m:sPre>",
                    r("6"),
                    r("14"),
                    e(&r("C"))
                ),
                "attach(C, bl: 6, tl: 14)",
            ),
            (r("a/b # $ \" ^ _ ~"), "a \\/ b \\# \\$ \\\" \\^ \\_ \\~"),
            (
                format!(
                    "<m:box>{}</m:box><m:phant><m:phantPr><m:show m:val=\"0\"/></m:phantPr>{}</m:phant>",
                    e(&r("x")),
                    e(&r("y"))
                ),
                "x",
            ),
        ];
        for (xml, typst) in cases {
            let (out, warnings) = omml(&xml);
            assert_eq!(out, typst, "{xml}");
            assert!(parses(&out), "{out}");
            assert!(warnings.is_empty(), "{xml}: {warnings:?}");
        }
    }

    #[test]
    fn unknown_elements_keep_their_text() {
        let (out, warnings) = omml(&format!("<m:foo>{}</m:foo> + 1", r("q")));
        assert_eq!(out, "\"q\"");
        assert_eq!(warnings[0].kind, WarningKind::Math);
        assert_eq!(warnings[0].example.as_deref(), Some("m:foo"));
    }

    #[test]
    fn odd_input_still_parses() {
        let odd = [
            r("(((").to_string(),
            format!("<m:f><m:num>{}</m:num><m:den/></m:f>", r(")],;[{")),
            "<m:sSup><m:e/><m:sup/></m:sSup>".to_owned(),
            format!(
                "<m:d><m:dPr><m:begChr m:val=\"\"/><m:endChr m:val=\"|\"/><m:sepChr m:val=\",\"/></m:dPr>{}{}</m:d>",
                e(&r("a")),
                e(&r("b"))
            ),
            format!(
                "<m:d><m:dPr><m:begChr m:val=\")\"/><m:endChr m:val=\"(\"/></m:dPr>{}</m:d>",
                e(&r("a"))
            ),
            format!(
                "<m:sSub>{}<m:sub>{}</m:sub></m:sSub>",
                e(&format!(
                    "<m:sSup>{}<m:sup>{}</m:sup></m:sSup>",
                    e(&r("x")),
                    r("2")
                )),
                r("i")
            ),
            format!(
                "<m:nary><m:naryPr><m:chr m:val=\"#\"/><m:limLoc m:val=\"subSup\"/></m:naryPr><m:sub>{}</m:sub><m:sup/>{}</m:nary>",
                r(","),
                e("")
            ),
            "<m:m><m:mr><m:e/><m:e/></m:mr></m:m>".to_owned(),
            format!(
                "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                r("&amp;"),
                r("'")
            ),
            r("\u{2061}\u{2062} x '"),
            format!("<m:limLow>{}<m:lim/></m:limLow>", e(&r("a,b"))),
            format!(
                "<m:func><m:fName>{}</m:fName>{}</m:func>",
                r("foo"),
                e(&r("x"))
            ),
        ];
        for xml in odd {
            let (out, _) = omml(&xml);
            assert!(parses(&out), "{xml} → {out}");
        }
        let deep = "<m:f><m:num>".repeat(50) + &r("x") + &"</m:num></m:f>".repeat(50);
        assert!(parses(&omml(&deep).0));
    }
}
