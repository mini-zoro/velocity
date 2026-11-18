//! Synthetic differential coverage of current-file Find through the App path.
//! Adopted from the independent verifier's harness. Use FIND_DIFF_SEED, FIND_DIFF_CASES,
//! or FIND_DIFF_ONLY to replay a run. The oracle searches contiguous UTF-8 with regex-automata.
use super::*;
use regex_automata::{Input as MInput, meta::Regex as MRegex};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use vel_platform::Size;

// ---------------------------------------------------------------- RNG
#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        let mut r = Rng((seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03) | 1);
        for _ in 0..4 {
            r.next();
        }
        r
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

// ---------------------------------------------------------------- documents
const WORDS: &[&str] = &[
    "hit", "Hit", "HIT", "hits", "foo", "bar", "foobar", "foo_bar", "x", "a", "aa", "aaa", "the",
    "cat", "Cat", "CATS", "fn", "let", "self", "x1", "42", "3.14", "a-b", "café", "naïve",
    "Straße", "STRASSE", "abab",
];
const CJK: &[&str] = &[
    "前",
    "後",
    "猫",
    "字",
    "中文",
    "搜索",
    "搜索结果",
    "日本語",
    "한국어",
    "漢字",
    "ひらがな",
];
const EMOJI: &[&str] = &[
    "🦀",
    "👍🏽",
    "👨\u{200d}👩\u{200d}👧",
    "🇨🇳",
    "❤\u{fe0f}",
    "😀",
];
const MARKS: &[&str] = &[
    "e\u{301}",
    "a\u{308}",
    "n\u{303}",
    "\u{301}",
    "o\u{302}\u{323}",
    "é",
];
const CASES: &[&str] = &[
    "ß", "ẞ", "SS", "ss", "Σ", "σ", "ς", "İ", "i", "ı", "I", "\u{212A}", "k", "K", "Ａ", "ａ", "ǅ",
    "ﬀ", "ΐ",
];
const PUNCT: &[&str] = &[
    ".", "(", ")", "[", "]", "*", "+", "?", "\\", "$", "^", "|", "-", "_", "{", "}", "/", "#",
    "\"", "'", ",", ";", ":",
];
const SPACES: &[&str] = &[" ", " ", " ", "\t", "  ", "\u{3000}", "\u{a0}"];

fn token(rng: &mut Rng) -> &'static str {
    match rng.below(100) {
        0..=44 => rng.pick(WORDS),
        45..=59 => rng.pick(CJK),
        60..=64 => rng.pick(EMOJI),
        65..=71 => rng.pick(MARKS),
        72..=79 => rng.pick(CASES),
        80..=89 => rng.pick(PUNCT),
        _ => rng.pick(SPACES),
    }
}
fn gen_line(rng: &mut Rng, long: bool) -> String {
    let n = if long {
        rng.range(2_000, 40_000)
    } else {
        rng.range(0, 14)
    };
    let mut s = String::new();
    for i in 0..n {
        if i > 0 && rng.chance(0.6) {
            s.push(' ');
        }
        s.push_str(token(rng));
    }
    s
}
fn document(rng: &mut Rng) -> String {
    let lines = match rng.below(100) {
        0..=29 => rng.range(1, 5),
        30..=64 => rng.range(5, 300),
        65..=89 => rng.range(300, 5000),
        _ => rng.range(5000, 50000),
    };
    let ending = rng.below(10);
    let mut longs = 0;
    let mut s = String::new();
    for i in 0..lines {
        if i > 0 {
            let crlf = match ending {
                0..=5 => false,
                6..=8 => true,
                _ => rng.chance(0.5),
            };
            s.push_str(if crlf { "\r\n" } else { "\n" });
        }
        let long = longs < 3 && rng.chance(if lines < 10 { 0.15 } else { 0.003 });
        if long {
            longs += 1;
        }
        s.push_str(&gen_line(rng, long));
        if s.len() > 6_000_000 {
            break;
        }
    }
    if rng.chance(0.5) {
        s.push('\n');
    }
    s
}
fn snap(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}
fn sample_literal(rng: &mut Rng, text: &str) -> Option<String> {
    for _ in 0..20 {
        if text.is_empty() {
            return None;
        }
        let start = snap(text, rng.below(text.len()));
        let want = rng.range(1, 10);
        let q: String = text[start..]
            .chars()
            .take_while(|c| *c != '\n' && *c != '\r')
            .take(want)
            .collect();
        if !q.is_empty() {
            return Some(q);
        }
    }
    None
}
fn mangle_case(rng: &mut Rng, q: &str) -> String {
    q.chars()
        .flat_map(|c| match rng.below(3) {
            0 => c.to_uppercase().collect::<Vec<_>>(),
            1 => c.to_lowercase().collect::<Vec<_>>(),
            _ => vec![c],
        })
        .collect()
}
fn sample_query(rng: &mut Rng, text: &str, regex: bool) -> String {
    let roll = rng.below(100);
    if regex && roll < 35 {
        let w = sample_literal(rng, text).unwrap_or_else(|| "hit".into());
        let w2 = sample_literal(rng, text).unwrap_or_else(|| "cat".into());
        return match rng.below(12) {
            0 => format!("{}|{}", regex::escape(&w), regex::escape(&w2)),
            1 => r"\w+".into(),
            2 => r"[0-9]+".into(),
            3 => format!("^{}", regex::escape(&w)),
            4 => format!("{}$", regex::escape(&w)),
            5 => ".".into(),
            6 => "h.t".into(),
            7 => r"\p{Han}+".into(),
            8 => format!(r"\b{}\b", regex::escape(&w)),
            9 => "a*".into(),
            10 => r"\s+".into(),
            _ => format!("({})+", regex::escape(&w)),
        };
    }
    if (35..42).contains(&roll) {
        return format!("zq{}#", rng.below(1000));
    }
    if (42..52).contains(&roll) {
        return rng
            .pick(&[
                "a", " ", "e", "\u{301}", "前", "🦀", "ß", "σ", "i", "k", "\t", ".", "x",
            ])
            .to_string();
    }
    let mut q = sample_literal(rng, text).unwrap_or_else(|| "hit".into());
    if rng.chance(0.3) {
        q = mangle_case(rng, &q);
    }
    q
}

// ---------------------------------------------------------------- reference
#[derive(Clone, Copy, Debug, PartialEq)]
struct Opts {
    regex: bool,
    case: bool,
    whole: bool,
    wrap: bool,
}
fn reference(
    text: &str,
    query: &str,
    o: Opts,
    scope: Option<&[Range<usize>]>,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    let all = 0..text.len();
    let ranges = scope.unwrap_or(std::slice::from_ref(&all));
    let src = if o.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let src = if o.whole {
        format!(r"\b(?:{src})\b")
    } else {
        src
    };
    let pat = format!("(?m){}(?:{src})", if o.case { "" } else { "(?i)" });
    let re = MRegex::new(&pat).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for r in ranges {
        for m in re.find_iter(MInput::new(text).span(r.clone())) {
            out.push(m.start()..m.end());
        }
    }
    out.sort_by_key(|r| (r.start, r.end));
    out.dedup();
    Ok(out)
}
#[derive(Default)]
struct WordChars {
    cache: HashMap<char, bool>,
    re: Option<regex::Regex>,
}
impl WordChars {
    fn is_word(&mut self, c: char) -> bool {
        let re = self
            .re
            .get_or_insert_with(|| regex::Regex::new(r"^\w$").unwrap())
            .clone();
        *self
            .cache
            .entry(c)
            .or_insert_with(|| re.is_match(c.encode_utf8(&mut [0; 4])))
    }
    fn boundary(&mut self, text: &str, i: usize) -> bool {
        let before = text[..i]
            .chars()
            .next_back()
            .is_some_and(|c| self.is_word(c));
        let after = text[i..].chars().next().is_some_and(|c| self.is_word(c));
        before != after
    }
}
/// Independent literal scanner (case-sensitive), to validate the reference itself.
fn literal_scan(
    text: &str,
    q: &str,
    whole: bool,
    ranges: &[Range<usize>],
    w: &mut WordChars,
) -> Vec<Range<usize>> {
    let mut out = vec![];
    if q.is_empty() {
        return out;
    }
    for r in ranges {
        let mut i = r.start;
        while i + q.len() <= r.end {
            if text.is_char_boundary(i)
                && text.as_bytes()[i..].starts_with(q.as_bytes())
                && (!whole || (w.boundary(text, i) && w.boundary(text, i + q.len())))
            {
                out.push(i..i + q.len());
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out.sort_by_key(|r| (r.start, r.end));
    out.dedup();
    out
}
fn ref_next(
    m: &[Range<usize>],
    caret: usize,
    current: &Range<usize>,
    backwards: bool,
    wrap: bool,
) -> Option<Range<usize>> {
    let valid = |r: &&Range<usize>| *r != current;
    let found = if backwards {
        m.iter()
            .rev()
            .filter(valid)
            .find(|r| r.end <= caret)
            .or_else(|| {
                if wrap {
                    m.iter().rev().find(valid)
                } else {
                    None
                }
            })
    } else {
        m.iter()
            .filter(valid)
            .find(|r| r.start >= caret)
            .or_else(|| if wrap { m.iter().find(valid) } else { None })
    };
    found
        .cloned()
        .or_else(|| (wrap && m.contains(current)).then(|| current.clone()))
}
fn expected_regions(matches: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for hit in matches {
        if let Some(last) = out.last_mut()
            && (hit.start < last.end
                || (hit.start == last.end && (hit.is_empty() || last.start == last.end)))
        {
            // A caret on a selected boundary belongs to that region. Adjacent
            // nonempty matches remain separate editing targets.
            last.end = last.end.max(hit.end);
        } else {
            out.push(hit.clone());
        }
    }
    out
}

// ---------------------------------------------------------------- App harness
fn fapp(text: &str) -> App {
    let mut a = app(text, "Plain Text", None);
    a.view.size = Size {
        width: 800.,
        height: 400.,
    };
    a.workspace.editor_mut().set_caret(0, false);
    set(&mut a, "find_selected_text", json!(false));
    set(&mut a, "close_find_after_find_all", json!(false));
    a
}
fn pf(a: &mut App) {
    let viewport = a.viewport();
    a.panels.poll(&mut a.workspace, viewport);
    if std::mem::take(&mut a.panels.reveal) {
        a.view.reveal(a.workspace.editor());
    }
}
fn settle(a: &mut App) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        pf(a);
        if !a.panels.busy() {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("Find did not settle within 60 s".into());
        }
        std::thread::yield_now();
    }
}
fn typ(a: &mut App, value: &str) -> bool {
    let viewport = a.viewport();
    let ok = a.panels.text_input(
        &mut a.workspace,
        TextInput::Insert {
            text: value.into(),
            replacement: None,
        },
        viewport,
    );
    a.changed_state(Instant::now(), false);
    ok
}
fn hits(a: &App) -> Vec<Range<usize>> {
    a.panels.visible_highlights().to_vec()
}
fn regions(a: &App) -> Vec<Range<usize>> {
    a.workspace
        .editor()
        .regions()
        .iter()
        .map(|r| r.to_range())
        .collect()
}
fn sel(a: &App) -> Range<usize> {
    a.workspace.editor().selection()
}
fn painted(a: &mut App) -> Vec<String> {
    let scene = a.scene();
    scene
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .map(|run| run.text.clone())
        .collect()
}
fn describe(text: &str, r: &Range<usize>) -> String {
    let s = snap(text, r.start.saturating_sub(12));
    let e = snap(text, (r.end + 12).min(text.len()));
    format!(
        "{r:?} {:?} in …{:?}…",
        &text[snap(text, r.start)..snap(text, r.end)],
        &text[s..e]
    )
}
fn diff(text: &str, got: &[Range<usize>], want: &[Range<usize>]) -> String {
    let g: HashSet<_> = got.iter().cloned().collect();
    let w: HashSet<_> = want.iter().cloned().collect();
    let mut missing: Vec<_> = want.iter().filter(|r| !g.contains(*r)).cloned().collect();
    let mut extra: Vec<_> = got.iter().filter(|r| !w.contains(*r)).cloned().collect();
    missing.sort_by_key(|r| r.start);
    extra.sort_by_key(|r| r.start);
    format!(
        "got {} want {}; missing {} (first: {}); extra {} (first: {})",
        got.len(),
        want.len(),
        missing.len(),
        missing.first().map_or("-".into(), |r| describe(text, r)),
        extra.len(),
        extra.first().map_or("-".into(), |r| describe(text, r)),
    )
}

#[derive(Default, Debug)]
struct Stats {
    cases: usize,
    bytes: usize,
    max_bytes: usize,
    max_lines: usize,
    matches: usize,
    zero: usize,
    regex: usize,
    regex_invalid: usize,
    insel: usize,
    manual_insel: usize,
    crlf: usize,
    wrapped: usize,
    folded: usize,
    incremental: usize,
    replace: usize,
    race: usize,
    nav_steps: usize,
    full_cycles: usize,
    find_all: usize,
    merged_boundary_carets: usize,
    literal_crosscheck: usize,
    edits: usize,
    undo: usize,
    status_checked: usize,
    restore: usize,
    input_altered: usize,
    rows_checked: usize,
}

fn line_of(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

fn run_case(seed: u64, st: &mut Stats, notes: &mut Vec<String>) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let text = document(&mut rng);
    let lines = text.matches('\n').count() + 1;
    st.cases += 1;
    st.bytes += text.len();
    st.max_bytes = st.max_bytes.max(text.len());
    st.max_lines = st.max_lines.max(lines);
    if text.contains("\r\n") {
        st.crlf += 1;
    }
    let mut log = vec![format!("seed={seed} bytes={} lines={lines}", text.len())];
    macro_rules! fail {
        ($($t:tt)*) => {
            return Err(format!("{} | {}", log.join("; "), format!($($t)*)))
        };
    }
    let mut a = fapp(&text);
    let small = text.len() < 300_000;
    if rng.chance(0.25) {
        set(&mut a, "word_wrap", json!(true));
        a.view.size.width = [320., 480., 800.][rng.below(3)];
        st.wrapped += 1;
        log.push("wrap".into());
    }
    let auto = rng.chance(0.4);
    if auto {
        set(&mut a, "auto_find_in_selection", json!(true));
        log.push("auto_find_in_selection".into());
    }
    if rng.chance(0.3) {
        set(&mut a, "find_selected_text", json!(true));
        log.push("find_selected_text".into());
    }
    let len = text.len();
    if len > 0 && rng.chance(0.2) {
        let mut folds = vec![];
        for _ in 0..rng.range(1, 3) {
            let s = snap(&text, rng.below(len));
            let e = snap(&text, s + rng.range(1, 3000));
            if e > s {
                folds.push(Region::new(s, e));
            }
        }
        log.push(format!(
            "folds={:?}",
            folds.iter().map(|r| r.to_range()).collect::<Vec<_>>()
        ));
        a.workspace.editor_mut().set_folded(folds);
        st.folded += 1;
    }
    // Selection.
    let pos = |rng: &mut Rng| snap(&text, rng.below(len + 1));
    let kind = rng.below(10);
    let mut sel_regions: Vec<Region> = match kind {
        0..=3 => {
            let p = pos(&mut rng);
            vec![Region::new(p, p)]
        }
        4..=5 => {
            let p = pos(&mut rng);
            let e = text[p..]
                .char_indices()
                .take_while(|(_, c)| *c != '\n' && *c != '\r')
                .take(rng.range(1, 20))
                .last()
                .map_or(p, |(i, c)| p + i + c.len_utf8());
            vec![Region::new(p, e)]
        }
        6..=7 => {
            let p = pos(&mut rng);
            let q = pos(&mut rng);
            vec![Region::new(p.min(q), p.max(q))]
        }
        _ => (0..rng.range(2, 5))
            .map(|_| {
                let p = pos(&mut rng);
                let q = snap(&text, p + rng.range(0, 400));
                Region::new(p, q)
            })
            .collect(),
    };
    if rng.chance(0.3) {
        sel_regions = sel_regions
            .into_iter()
            .map(|r| Region::new(r.b, r.a))
            .collect();
    }
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions(sel_regions));
    let open_regions = regions(&a);
    let mut scope: Vec<Range<usize>> = a
        .workspace
        .editor()
        .regions()
        .iter()
        .filter(|r| !r.empty())
        .map(|r| r.to_range())
        .collect();
    log.push(format!(
        "selection={:?} primary={:?}",
        if open_regions.len() > 6 {
            open_regions[..6].to_vec()
        } else {
            open_regions.clone()
        },
        sel(&a)
    ));
    let multiline = scope
        .iter()
        .any(|r| line_of(&text, r.start) != line_of(&text, r.end));
    // Panel.
    let panel_kind = rng.below(10);
    let open_key = match panel_kind {
        0..=7 => "super+f",
        8 => "super+i",
        _ => "super+alt+f",
    };
    let incremental = panel_kind == 8;
    let replace = panel_kind == 9;
    if incremental {
        st.incremental += 1;
    }
    if replace {
        st.replace += 1;
    }
    log.push(open_key.into());
    key(&mut a, open_key);
    let origin = sel(&a).start;
    let mut model_insel = auto && multiline;
    let mut manual = false;
    if a.panels.in_selection != model_insel {
        fail!(
            "in_selection after open = {} expected {}",
            a.panels.in_selection,
            model_insel
        );
    }
    let mut o = Opts {
        regex: false,
        case: false,
        whole: false,
        wrap: true,
    };
    let race = !incremental && rng.chance(0.4);
    let toggle = |a: &mut App,
                  rng: &mut Rng,
                  o: &mut Opts,
                  model: &mut bool,
                  manual: &mut bool,
                  log: &mut Vec<String>| {
        match rng.below(5) {
            0 => {
                key(a, "alt+super+c");
                o.case = !o.case;
                log.push("toggle case".into());
            }
            1 => {
                key(a, "alt+super+w");
                o.whole = !o.whole;
                log.push("toggle whole".into());
            }
            2 => {
                key(a, "alt+super+r");
                o.regex = !o.regex;
                log.push("toggle regex".into());
            }
            3 => {
                named(a, "toggle_find_option", json!({"option":"wrap"}));
                o.wrap = !o.wrap;
                log.push("toggle wrap".into());
            }
            _ => {
                named(a, "toggle_find_option", json!({"option":"in_selection"}));
                *model = !*model;
                *manual = true;
                log.push("toggle in_selection".into());
            }
        }
    };
    for _ in 0..rng.range(0, 3) {
        toggle(
            &mut a,
            &mut rng,
            &mut o,
            &mut model_insel,
            &mut manual,
            &mut log,
        );
    }
    let mut query = sample_query(&mut rng, &text, o.regex);
    log.push(format!("query={query:?}"));
    if rng.chance(0.7) {
        for c in query.chars() {
            if !typ(&mut a, &c.to_string()) {
                fail!("panel input refused text");
            }
            if rng.chance(0.3) {
                pf(&mut a);
            }
        }
    } else if !typ(&mut a, &query) {
        fail!("panel input refused text");
    }
    let typed = a.panels.query.value();
    if typed != query {
        st.input_altered += 1;
        notes.push(format!(
            "seed={seed}: typed {query:?} but input holds {typed:?}"
        ));
        query = typed;
    }
    if !race {
        for _ in 0..rng.range(0, 2) {
            toggle(
                &mut a,
                &mut rng,
                &mut o,
                &mut model_insel,
                &mut manual,
                &mut log,
            );
        }
    }
    if a.panels.options.regex != o.regex
        || a.panels.options.case_sensitive != o.case
        || a.panels.options.whole_word != o.whole
        || a.panels.options.wrap != o.wrap
    {
        fail!("options mismatch {:?} vs {:?}", a.panels.options, o);
    }
    log.push(format!("opts={o:?} insel={model_insel} manual={manual}"));
    if o.regex {
        st.regex += 1;
    }
    if model_insel {
        st.insel += 1;
        if manual {
            st.manual_insel += 1;
        }
    }
    let scope_opt = |model: bool, scope: &Vec<Range<usize>>| -> Option<Vec<Range<usize>>> {
        model.then(|| scope.clone())
    };
    let expected = reference(&text, &query, o, scope_opt(model_insel, &scope).as_deref());
    let start_sel = sel(&a);
    let start_caret = a.workspace.editor().caret;
    let mut queued = vec![];
    if race {
        st.race += 1;
        for _ in 0..rng.range(1, 3) {
            let back = rng.chance(0.3);
            key(&mut a, if back { "shift+enter" } else { "enter" });
            queued.push(back);
        }
        log.push(format!("race keys backwards={queued:?}"));
    }
    settle(&mut a)?;
    if a.panels.in_selection != model_insel {
        fail!(
            "in_selection = {} expected {}",
            a.panels.in_selection,
            model_insel
        );
    }
    let mut m = match expected {
        Ok(m) => m,
        Err(_) => {
            st.regex_invalid += 1;
            if !hits(&a).is_empty() {
                fail!("invalid regex but highlights {:?}", hits(&a).len());
            }
            if a.panels.message.is_empty() {
                fail!("invalid regex without message");
            }
            return Ok(());
        }
    };
    let got = hits(&a);
    if got != m {
        if std::env::var("FIND_DIFF_DEBUG").is_ok() {
            let gs: HashSet<_> = got.iter().cloned().collect();
            for r in m.iter().filter(|r| !gs.contains(*r)) {
                println!(
                    "DEBUG missing {} scope-range {:?}",
                    describe(&text, r),
                    scope.iter().find(|s| s.start <= r.start && r.end <= s.end)
                );
            }
            println!(
                "DEBUG scope {:?} got-first {:?} got-last {:?}",
                scope,
                got.first(),
                got.last()
            );
            println!("DEBUG regions now {:?} sel {:?}", regions(&a), sel(&a));
        }
        let class = if engine_check(
            &text,
            &query,
            o,
            if model_insel { Some(&scope) } else { None },
        )
        .is_some()
        {
            "ENGINE(regex-cursor)"
        } else {
            "PLUMBING"
        };
        fail!("{class} highlights differ: {}", diff(&text, &got, &m));
    }
    st.matches += m.len();
    if m.is_empty() {
        st.zero += 1;
    }
    if o.case && !o.regex {
        let all = 0..text.len();
        let ranges = if model_insel {
            &scope
        } else {
            std::slice::from_ref(&all)
        };
        let mut w = WordChars::default();
        let lit = literal_scan(&text, &query, o.whole, ranges, &mut w);
        if lit != m {
            notes.push(format!(
                "seed={seed}: literal scanner disagrees with regex reference: {}",
                diff(&text, &lit, &m)
            ));
        } else {
            st.literal_crosscheck += 1;
        }
    }
    // Preview / queued navigation.
    let previews = !model_insel && !replace && !query.is_empty();
    let mut state: Option<Range<usize>> = if previews {
        m.iter()
            .find(|r| r.start >= origin)
            .or_else(|| if o.wrap { m.first() } else { None })
            .cloned()
    } else {
        Some(start_sel.clone())
    };
    let mut state_caret = if previews {
        state.as_ref().map(|r| r.end)
    } else {
        Some(start_caret)
    };
    if !previews && race {
        // start state is the selection before the race; fine.
    }
    if let (Some(s), Some(c)) = (state.clone(), state_caret) {
        let mut cur = s;
        let mut caret = c;
        let mut known = true;
        for back in &queued {
            if let Some(n) = ref_next(&m, caret, &cur, *back, o.wrap) {
                cur = n.clone();
                caret = n.end;
            } else if previews && m.is_empty() {
                known = false;
            }
        }
        if known && !(previews && m.is_empty()) && sel(&a) != cur {
            fail!(
                "selection after typing/queued keys = {:?}, expected {:?} (origin {origin})",
                describe(&text, &sel(&a)),
                describe(&text, &cur)
            );
        }
        state = Some(cur);
        state_caret = Some(caret);
    }
    let _ = (&state, &state_caret);
    // Status text (small documents only).
    if small && rng.chance(0.3) && !query.is_empty() && (o.wrap || queued.is_empty()) {
        let expected_status = if m.is_empty() {
            let mut flags = vec![];
            if o.case {
                flags.push("Match Case on");
            }
            if o.whole {
                flags.push("Whole Word on");
            }
            if model_insel {
                flags.push(if scope.is_empty() {
                    "In Selection on; no selection"
                } else {
                    "In Selection on"
                });
            }
            if flags.is_empty() {
                "No matches".to_string()
            } else {
                format!("No matches ({})", flags.join(", "))
            }
        } else {
            format!(
                "{} {}",
                m.len(),
                if m.len() == 1 { "match" } else { "matches" }
            )
        };
        let runs = painted(&mut a);
        if !runs.iter().any(|r| r == &expected_status) {
            fail!(
                "status {expected_status:?} not painted; message {:?}",
                a.panels.message
            );
        }
        st.status_checked += 1;
    }
    // Wrapped-row highlight painting.
    if small && a.workspace.editor().settings.bool("word_wrap", false) && m.len() > 1 {
        let _ = a.scene();
        let e = a.workspace.editor();
        let color = crate::view::color(
            &e.language.borrow().as_ref().unwrap().scheme,
            "find_highlight",
        );
        let selected_now = regions(&a);
        let folds: Vec<_> = a
            .workspace
            .editor()
            .folded()
            .iter()
            .map(|r| r.to_range())
            .collect();
        let visible: Vec<_> = a
            .view
            .find_matches
            .iter()
            .filter(|r| !r.is_empty())
            .filter(|r| {
                !selected_now
                    .iter()
                    .any(|s| s.start < r.end && r.start < s.end)
            })
            .filter(|r| !folds.iter().any(|f| f.start < r.end && r.start < f.end))
            .cloned()
            .collect();
        let scene = a.scene();
        let e = a.workspace.editor();
        let vp = a.view.viewport(e);
        for r in visible
            .iter()
            .filter(|r| r.start >= vp.start && r.end <= vp.end)
            .take(5)
        {
            let g = e
                .text
                .prev_grapheme_boundary(e.text.next_grapheme_boundary(r.start));
            let target = a.view.rect_at(e, g);
            let ok = scene
                .layers
                .iter()
                .flat_map(|layer| &layer.rects)
                .any(|rect| {
                    rect.color == color
                        && (rect.rect.y - target.y).abs() < a.view.line_height * 0.75
                });
            if !ok && std::env::var("FIND_DIFF_DEBUG").is_ok() {
                let ys: Vec<_> = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.rects)
                    .filter(|rect| rect.color == color)
                    .map(|rect| (rect.rect.x, rect.rect.y))
                    .collect();
                let runs: Vec<_> = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .filter(|t| {
                        (t.baseline as f64) > target.y + 20.
                            && (t.baseline as f64) <= target.y + 40.
                    })
                    .map(|t| {
                        (
                            t.x,
                            t.baseline,
                            t.text.chars().take(12).collect::<String>(),
                            t.clip,
                        )
                    })
                    .collect();
                println!("DEBUG runs on row: {runs:?}");
                let ctx: String = text[snap(&text, r.start.saturating_sub(6))..r.start].to_string();
                let holder: Vec<_> = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .filter(|t| t.text.contains(ctx.trim()) && !ctx.trim().is_empty())
                    .map(|t| (t.x, t.baseline, t.text.clone()))
                    .collect();
                println!("DEBUG ctx {ctx:?} runs containing it: {holder:?}");
                let top: Vec<_> = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .filter(|t| t.baseline < 60. && t.baseline > 0.)
                    .map(|t| (t.x, t.baseline, t.text.chars().take(10).collect::<String>()))
                    .collect();
                println!("DEBUG top runs: {top:?}");
                println!(
                    "DEBUG view scroll_y={} content={:?} rect_at(0)={:?} rect_at(630)={:?}",
                    a.view.scroll_y,
                    a.content,
                    a.view.rect_at(e, 0),
                    a.view.rect_at(e, 630)
                );
                println!(
                    "DEBUG row target={target:?} line_height={} highlight rects={ys:?} find_matches={:?} vp={vp:?} sel={:?} folds={:?}",
                    a.view.line_height,
                    a.view.find_matches,
                    regions(&a),
                    a.workspace.editor().folded()
                );
            }
            if !ok
                && target.y >= 2. * a.view.line_height
                && target.y + a.view.line_height <= a.view.size.height
            {
                fail!(
                    "highlight for {} not painted on its row (y={})",
                    describe(&text, r),
                    target.y
                );
            }
            st.rows_checked += 1;
        }
    }
    // The user changes the selection while the panel stays open (mouse drag / click).
    if !incremental && rng.chance(0.25) {
        let p = snap(&text, rng.below(len + 1));
        let q = match rng.below(3) {
            0 => p,
            1 => snap(&text, p + rng.range(1, 30)),
            _ => snap(&text, rng.below(len + 1)),
        };
        a.workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(p, q)]));
        settle(&mut a)?;
        let r = p.min(q)..p.max(q);
        scope = if r.is_empty() {
            vec![]
        } else {
            vec![r.clone()]
        };
        let multi = !r.is_empty() && line_of(&text, r.start) != line_of(&text, r.end);
        if !manual {
            model_insel = model_insel && multi;
        }
        log.push(format!("user selection {p}..{q} -> insel={model_insel}"));
        if a.panels.in_selection != model_insel {
            fail!(
                "in_selection after user selection = {} expected {}",
                a.panels.in_selection,
                model_insel
            );
        }
        m = match reference(&text, &query, o, model_insel.then_some(&scope[..])) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        let got = hits(&a);
        if got != m {
            let class = if engine_check(
                &text,
                &query,
                o,
                if model_insel { Some(&scope) } else { None },
            )
            .is_some()
            {
                "ENGINE(regex-cursor)"
            } else {
                "PLUMBING"
            };
            fail!(
                "{class} highlights after user selection differ: {}",
                diff(&text, &got, &m)
            );
        }
        if sel(&a) != r {
            fail!("user selection was changed by Find: {:?}", sel(&a));
        }
    }
    if incremental {
        // Enter: next match and close.
        let before = sel(&a);
        let caret = a.workspace.editor().caret;
        let want = ref_next(&m, caret, &before, false, o.wrap);
        key(&mut a, "enter");
        settle(&mut a)?;
        if a.panels.panel.is_some() {
            fail!("Cmd+I Enter left the panel open");
        }
        let after = sel(&a);
        if let Some(w) = want
            && after != w
        {
            fail!(
                "Cmd+I Enter selected {:?} expected {:?}",
                describe(&text, &after),
                describe(&text, &w)
            );
        }
        return Ok(());
    }
    // Enter cycle.
    if !m.is_empty() {
        let n = m.len();
        let steps = if n <= 400 { n + 1 } else { 40 };
        let mut visited = HashSet::new();
        for i in 0..steps {
            let before = sel(&a);
            let caret = a.workspace.editor().caret;
            let want = ref_next(&m, caret, &before, false, o.wrap);
            key(&mut a, "enter");
            pf(&mut a);
            let after = sel(&a);
            st.nav_steps += 1;
            match &want {
                Some(w) if after != *w => fail!(
                    "Enter #{i} from {:?} selected {:?}, expected {:?}",
                    before,
                    describe(&text, &after),
                    describe(&text, w)
                ),
                None if after != before => fail!("Enter #{i} moved without a later match"),
                _ => {}
            }
            if want.is_none() {
                break;
            }
            if small && i < 25 {
                let _ = a.scene();
                let e = a.workspace.editor();
                let vp = a.view.viewport(e);
                let folded = e
                    .folded()
                    .iter()
                    .any(|f| f.begin() < after.start && after.start < f.end());
                if !(vp.start <= after.start && after.start <= vp.end) || folded {
                    fail!(
                        "Enter #{i} selected {} but it is not revealed (viewport {vp:?}, folded {folded})",
                        describe(&text, &after)
                    );
                }
            }
            visited.insert(after);
        }
        if o.wrap && n <= 400 {
            st.full_cycles += 1;
            if let Some(missed) = m.iter().find(|r| !visited.contains(*r)) {
                fail!("Enter cycle never visited {}", describe(&text, missed));
            }
        }
        for i in 0..rng.range(1, 4) {
            let before = sel(&a);
            let caret = a.workspace.editor().caret;
            let want = ref_next(&m, caret, &before, true, o.wrap);
            key(&mut a, "shift+enter");
            pf(&mut a);
            let after = sel(&a);
            st.nav_steps += 1;
            if let Some(w) = &want
                && after != *w
            {
                fail!(
                    "Shift+Enter #{i} selected {:?}, expected {:?}",
                    describe(&text, &after),
                    describe(&text, w)
                );
            }
        }
        settle(&mut a)?;
        let got = hits(&a);
        if got != m {
            fail!(
                "highlights changed after navigation (scope drift?): {}",
                diff(&text, &got, &m)
            );
        }
        if a.panels.in_selection != model_insel {
            fail!("in_selection changed by navigation");
        }
    }
    // Find All (alt+enter closes the panel).
    let before = regions(&a);
    key(&mut a, "alt+enter");
    settle(&mut a)?;
    st.find_all += 1;
    if a.panels.panel.is_some() {
        fail!("alt+enter did not close the panel");
    }
    let after = regions(&a);
    if m.is_empty() {
        if after != before {
            fail!("Find All with no matches changed the selection");
        }
    } else {
        let want = expected_regions(&m);
        if want.len() < m.len() {
            st.merged_boundary_carets += 1;
        }
        if after != want {
            fail!("Find All selection differs: {}", diff(&text, &after, &want));
        }
    }
    // Edits while the panel is open (whole-file scope).
    if !query.is_empty() && rng.chance(0.5) {
        let p = snap(&text, rng.below(len + 1));
        a.workspace.editor_mut().set_caret(p, false);
        key(&mut a, "super+f");
        // Reopening may seed the query from a selection; it is a caret here.
        if a.panels.query.value() != query {
            fail!("query not kept across reopen: {:?}", a.panels.query.value());
        }
        settle(&mut a)?;
        if a.panels.in_selection {
            fail!("in_selection active after reopening with a caret");
        }
        let whole_ref = reference(&text, &query, o, None).unwrap();
        let got = hits(&a);
        if got != whole_ref {
            fail!(
                "reopen highlights differ: {}",
                diff(&text, &got, &whole_ref)
            );
        }
        let edit = rng.below(3);
        let at = snap(&text, rng.below(len + 1));
        let insert_text = match edit {
            0 => query.clone(),
            1 => token(&mut rng).to_string(),
            _ => String::new(),
        };
        if edit < 2 {
            a.workspace.editor_mut().set_caret(at, false);
            a.workspace
                .editor_mut()
                .run("insert", &json!({"characters": insert_text}))
                .map_err(|e| format!("insert: {e}"))?;
        } else {
            let e = snap(&text, at + rng.range(1, 50));
            a.workspace
                .editor_mut()
                .set_selection(Selection::from_regions([Region::new(at, e)]));
            a.workspace
                .editor_mut()
                .run("left_delete", &json!({}))
                .map_err(|e| format!("delete: {e}"))?;
        }
        st.edits += 1;
        log.push(format!("edit kind={edit} at={at} text={insert_text:?}"));
        let new_text = a.workspace.editor().text.to_string();
        let new_ref = reference(&new_text, &query, o, None).unwrap();
        // Before the next poll nothing stale may be painted.
        let stale = a.panels.current_highlights(&a.workspace).to_vec();
        if !stale.is_empty() && stale != new_ref {
            fail!(
                "stale highlights offered before poll: {}",
                diff(&new_text, &stale, &new_ref)
            );
        }
        let _ = a.scene();
        let new_set: HashSet<_> = new_ref.iter().cloned().collect();
        if let Some(bad) = a.view.find_matches.iter().find(|r| !new_set.contains(*r)) {
            fail!(
                "painted stale match {} after edit",
                describe(&new_text, bad)
            );
        }
        settle(&mut a)?;
        let got = hits(&a);
        if got != new_ref {
            // Classify using the edited rope itself (its chunking is what the worker searched).
            let rope = a.workspace.editor().text.clone();
            let options = vel_search::text::Options {
                regex: o.regex,
                case_sensitive: o.case,
                whole_word: o.whole,
                wrap: o.wrap,
                preserve_case: false,
            };
            let engine = vel_search::text::Search::new(&query, options)
                .map(|s| s.find(&rope, &[]))
                .unwrap_or_default();
            let mut off = 0;
            let ends: Vec<usize> = rope
                .chunks()
                .map(|c| {
                    off += c.len();
                    off
                })
                .take(8)
                .collect();
            let class = if engine != new_ref {
                "ENGINE(regex-cursor on edited rope)"
            } else {
                "PLUMBING"
            };
            fail!(
                "{class} highlights after edit differ: {} chunk_ends={ends:?}",
                diff(&new_text, &got, &new_ref)
            );
        }
        // Undo.
        a.workspace
            .editor_mut()
            .run("undo", &json!({}))
            .map_err(|e| format!("undo: {e}"))?;
        st.undo += 1;
        let undone = a.workspace.editor().text.to_string();
        if undone != text {
            notes.push(format!("seed={seed}: undo did not restore text exactly"));
        } else {
            settle(&mut a)?;
            let got = hits(&a);
            if got != whole_ref {
                fail!(
                    "highlights after undo differ: {}",
                    diff(&text, &got, &whole_ref)
                );
            }
        }
        // Find All after edits.
        key(&mut a, "alt+enter");
        settle(&mut a)?;
        let after = regions(&a);
        let cur = a.workspace.editor().text.to_string();
        let r = reference(&cur, &query, o, None).unwrap();
        if !r.is_empty() && after != expected_regions(&r) {
            fail!(
                "Find All after undo differs: {}",
                diff(&cur, &after, &expected_regions(&r))
            );
        }
    }
    // Session restore keeps only the query (and history).
    if rng.chance(0.1) && !query.is_empty() {
        st.restore += 1;
        let mut session = vel_session::Window::default();
        a.panels.capture_session(&mut session);
        let cur = a.workspace.editor().text.to_string();
        let mut b = fapp(&cur);
        b.panels.restore_session(&b.workspace, &session);
        key(&mut b, "super+f");
        settle(&mut b)?;
        let ro = Opts {
            regex: b.panels.options.regex,
            case: b.panels.options.case_sensitive,
            whole: b.panels.options.whole_word,
            wrap: b.panels.options.wrap,
        };
        let q = b.panels.query.value();
        if let Ok(r) = reference(&cur, &q, ro, None) {
            let got = hits(&b);
            if got != r {
                fail!(
                    "restored session highlights differ: {}",
                    diff(&cur, &got, &r)
                );
            }
        }
    }
    let _ = &mut scope;
    Ok(())
}

#[test]
#[ignore = "seeded App differential with synthetic documents"]
fn find_differential_random() {
    let base: u64 = std::env::var("FIND_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let count: u64 = std::env::var("FIND_DIFF_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let only: Option<u64> = std::env::var("FIND_DIFF_ONLY")
        .ok()
        .and_then(|s| s.parse().ok());
    let mut st = Stats::default();
    let mut notes = vec![];
    let mut failures = vec![];
    let started = Instant::now();
    let seeds: Vec<u64> = match only {
        Some(s) => vec![s],
        None => (base..base + count).collect(),
    };
    for seed in seeds {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut local = Stats::default();
            let mut n = vec![];
            let r = run_case(seed, &mut local, &mut n);
            (r, local, n)
        }));
        match result {
            Ok((r, local, n)) => {
                macro_rules! add {
                    ($($f:ident),*) => { $( st.$f += local.$f; )* };
                }
                add!(
                    cases,
                    bytes,
                    matches,
                    zero,
                    regex,
                    regex_invalid,
                    insel,
                    manual_insel,
                    crlf,
                    wrapped,
                    folded,
                    incremental,
                    replace,
                    race,
                    nav_steps,
                    full_cycles,
                    find_all,
                    merged_boundary_carets,
                    literal_crosscheck,
                    edits,
                    undo,
                    status_checked,
                    restore,
                    input_altered,
                    rows_checked
                );
                st.max_bytes = st.max_bytes.max(local.max_bytes);
                st.max_lines = st.max_lines.max(local.max_lines);
                notes.extend(n);
                if let Err(e) = r {
                    println!("FAIL {e}");
                    failures.push(e);
                }
            }
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                println!("PANIC seed={seed}: {msg}");
                failures.push(format!("panic seed={seed}: {msg}"));
            }
        }
    }
    println!("FIND_DIFF_STATS {st:?} elapsed={:?}", started.elapsed());
    for n in notes.iter().take(40) {
        println!("NOTE {n}");
    }
    println!("FIND_DIFF_FAILURES {}", failures.len());
    assert!(failures.is_empty(), "{} failures", failures.len());
}

fn engine_check(text: &str, q: &str, o: Opts, scope: Option<&[Range<usize>]>) -> Option<String> {
    let options = vel_search::text::Options {
        regex: o.regex,
        case_sensitive: o.case,
        whole_word: o.whole,
        wrap: o.wrap,
        preserve_case: false,
    };
    let Ok(search) = vel_search::text::Search::new(q, options) else {
        return None;
    };
    let rope = vel_text::Text::from(text);
    let got = search.find(&rope, scope.unwrap_or(&[]));
    let want = reference(text, q, o, scope).ok()?;
    (got != want).then(|| diff(text, &got, &want))
}
fn cap(text: String, max: usize) -> String {
    let end = snap(&text, max);
    text[..end].to_string()
}

#[test]
#[ignore = "verifier randomized tab-switch differential"]
fn find_tab_switch_random() {
    let base: u64 = std::env::var("FIND_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let count: u64 = std::env::var("FIND_DIFF_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let mut failures = vec![];
    let mut checked = 0;
    for seed in base..base + count {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            let mut rng = Rng::new(seed ^ 0x7AB0);
            let ta = cap(document(&mut rng), 400_000);
            let tb_src = cap(document(&mut rng), 200_000);
            let mut a = fapp(&ta);
            let original = a.workspace.active;
            named(&mut a, "new_file", json!({}));
            a.workspace
                .editor_mut()
                .run("insert", &json!({"characters": tb_src}))
                .unwrap();
            let tb = a.workspace.editor().text.to_string();
            let other = a.workspace.active;
            a.workspace
                .editor_mut()
                .set_caret(snap(&tb, rng.below(tb.len() + 1)), false);
            a.workspace.select(original);
            a.sync_active_view();
            let auto = rng.chance(0.5);
            if auto {
                set(&mut a, "auto_find_in_selection", json!(true));
            }
            let p = snap(&ta, rng.below(ta.len() + 1));
            let q2 = snap(&ta, rng.below(ta.len() + 1));
            let range = p.min(q2)..p.max(q2);
            a.workspace
                .editor_mut()
                .set_selection(Selection::from_regions([Region::from(range.clone())]));
            key(&mut a, "super+f");
            let multiline =
                !range.is_empty() && line_of(&ta, range.start) != line_of(&ta, range.end);
            let mut insel = auto && multiline;
            if rng.chance(0.5) {
                named(
                    &mut a,
                    "toggle_find_option",
                    json!({"option":"in_selection"}),
                );
                insel = !insel;
            }
            let src = if rng.chance(0.5) { &ta } else { &tb };
            let query = sample_literal(&mut rng, src).unwrap_or_else(|| "hit".into());
            typ(&mut a, &query);
            settle(&mut a)?;
            let o = Opts {
                regex: false,
                case: false,
                whole: false,
                wrap: true,
            };
            let scope: Vec<_> = if range.is_empty() {
                vec![]
            } else {
                vec![range.clone()]
            };
            let want = reference(&ta, &query, o, insel.then_some(&scope[..])).unwrap();
            if hits(&a) != want {
                return Err(format!(
                    "seed={seed} tab A before switch: {}",
                    diff(&ta, &hits(&a), &want)
                ));
            }
            key(&mut a, "ctrl+tab");
            if a.workspace.active != other {
                return Err(format!("seed={seed} Ctrl+Tab did not switch"));
            }
            a.changed_state(Instant::now(), false);
            settle(&mut a)?;
            if a.panels.in_selection {
                return Err(format!("seed={seed} in_selection carried to tab B"));
            }
            let want_b = reference(&tb, &query, o, None).unwrap();
            if hits(&a) != want_b {
                return Err(format!(
                    "seed={seed} tab B: {}",
                    diff(&tb, &hits(&a), &want_b)
                ));
            }
            if !want_b.is_empty() {
                let before = sel(&a);
                let caret = a.workspace.editor().caret;
                let exp = ref_next(&want_b, caret, &before, false, true).unwrap();
                key(&mut a, "enter");
                settle(&mut a)?;
                if sel(&a) != exp {
                    return Err(format!(
                        "seed={seed} Enter on tab B selected {:?} expected {:?}",
                        sel(&a),
                        exp
                    ));
                }
            }
            named(&mut a, "next_view", json!({}));
            if a.workspace.active != original {
                return Err(format!("seed={seed} next_view back did not switch"));
            }
            a.changed_state(Instant::now(), false);
            settle(&mut a)?;
            if a.panels.in_selection {
                return Err(format!(
                    "seed={seed} in_selection active after switching back"
                ));
            }
            let want_a = reference(&ta, &query, o, None).unwrap();
            if hits(&a) != want_a {
                return Err(format!(
                    "seed={seed} tab A after switch back: {}",
                    diff(&ta, &hits(&a), &want_a)
                ));
            }
            key(&mut a, "alt+enter");
            settle(&mut a)?;
            if !want_a.is_empty() && regions(&a) != expected_regions(&want_a) {
                return Err(format!(
                    "seed={seed} Find All on A after switches: {}",
                    diff(&ta, &regions(&a), &expected_regions(&want_a))
                ));
            }
            Ok(())
        }));
        checked += 1;
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                println!("TABFAIL {e}");
                failures.push(e);
            }
            Err(_) => {
                println!("TABPANIC seed={seed}");
                failures.push(format!("panic {seed}"));
            }
        }
    }
    println!("TAB_SWITCH checked={checked} failures={}", failures.len());
    assert!(
        failures.is_empty(),
        "{} tab switch failures",
        failures.len()
    );
}

#[test]
#[ignore = "verifier randomized progressive-load differential"]
fn find_progressive_random() {
    let base: u64 = std::env::var("FIND_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let count: u64 = std::env::var("FIND_DIFF_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let dir = tempfile::tempdir().unwrap();
    let mut failures = 0;
    let mut checked = 0;
    for seed in base..base + count {
        let mut rng = Rng::new(seed ^ 0x9909);
        let mut doc = String::new();
        while doc.len() < 300_000 {
            doc.push_str(&document(&mut rng));
            doc.push('\n');
        }
        let doc = cap(doc, 3_000_000);
        let path = dir.path().join(format!("p{seed}.txt"));
        std::fs::write(&path, &doc).unwrap();
        let mut a = fapp("");
        a.workspace.settings.set(
            vel_config::settings::Layer::User,
            "large_file.load_bytes",
            json!(100_000),
        );
        a.workspace
            .open(crate::cli::FileLocation {
                path: path.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        if a.workspace.tabs[a.workspace.active].loading.is_none() {
            println!("PROG seed={seed}: not progressive");
            continue;
        }
        a.sync_active_view();
        key(&mut a, "super+f");
        let query = sample_literal(&mut rng, &doc).unwrap_or_else(|| "hit".into());
        for c in query.chars() {
            typ(&mut a, &c.to_string());
        }
        let action = rng.below(3);
        let enters = rng.range(0, 3);
        match action {
            0 => {
                for _ in 0..enters {
                    key(&mut a, "enter");
                }
            }
            1 => key(&mut a, "alt+enter"),
            _ => {}
        }
        // Partial snapshots arrive while Find is pending.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut polls = 0;
        while a.workspace.tabs[a.workspace.active].loading.is_some() {
            a.workspace.poll();
            pf(&mut a);
            polls += 1;
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        settle(&mut a).unwrap();
        checked += 1;
        let text = a.workspace.editor().text.to_string();
        let o = Opts {
            regex: false,
            case: false,
            whole: false,
            wrap: true,
        };
        let m = reference(&text, &query, o, None).unwrap();
        let mut fail = None;
        match action {
            1 => {
                if a.panels.panel.is_some() {
                    fail = Some("alt+enter during load left panel open".to_string());
                } else if !m.is_empty() && regions(&a) != expected_regions(&m) {
                    let class = if engine_check(&text, &query, o, None).is_some() {
                        "ENGINE"
                    } else {
                        "PLUMBING"
                    };
                    fail = Some(format!(
                        "{class} Find All during load: {}",
                        diff(&text, &regions(&a), &expected_regions(&m))
                    ));
                }
            }
            _ => {
                if hits(&a) != m {
                    let class = if engine_check(&text, &query, o, None).is_some() {
                        "ENGINE"
                    } else {
                        "PLUMBING"
                    };
                    fail = Some(format!(
                        "{class} highlights after load: {}",
                        diff(&text, &hits(&a), &m)
                    ));
                } else if !m.is_empty() {
                    let expect = m[enters.min(m.len() * 4) % m.len()].clone();
                    let expect = if action == 0 { expect } else { m[0].clone() };
                    if sel(&a) != expect {
                        fail = Some(format!(
                            "selection after load + {enters} Enter(s) = {:?}, expected {:?}",
                            sel(&a),
                            expect
                        ));
                    }
                }
            }
        }
        if let Some(f) = fail {
            failures += 1;
            println!(
                "PROGFAIL seed={seed} bytes={} query={query:?} action={action} enters={enters} polls={polls}: {f}",
                doc.len()
            );
        }
    }
    println!("PROGRESSIVE checked={checked} failures={failures}");
    assert_eq!(checked as u64, count);
    assert_eq!(failures, 0);
}
