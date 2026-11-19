//! Read literal command branches from the App's named-dispatch entry points.
//! The editor library supplies the text commands and explicit command kinds;
//! these window-side names also need to exist before configuration is loaded.
use std::{collections::BTreeSet, fs, io, path::Path};
use syn::{
    BinOp, Expr, ImplItem, Item, Lit, Pat, Token,
    parse::{Parse, ParseStream},
    visit::{self, Visit},
};

const ROUTES: &[(&str, &[&str])] = &[
    (
        "app.rs",
        &[
            "run_named_depth",
            "apply_named_action",
            "apply_action",
            "format_action",
            "select_tab_action",
        ],
    ),
    ("app_build.rs", &["apply_build_action"]),
    ("app_context.rs", &["context_action"]),
    ("app_mouse.rs", &["apply_view_action"]),
    (
        "app_tabs.rs",
        &["apply_save_action", "apply_tab_close_action"],
    ),
    ("panels.rs", &["action"]),
    ("release.rs", &["action"]),
    ("review_ui.rs", &["review_action"]),
    ("settings_entry.rs", &["apply_settings_action"]),
    ("text_ops.rs", &["macro_action"]),
    ("workbench.rs", &["workbench_action"]),
];

pub(super) fn generate(out: &Path) -> io::Result<()> {
    println!("cargo:rerun-if-changed=build/dispatch.rs");
    let mut names = Names::default();
    for (source, routes) in ROUTES {
        let path = format!("src/{source}");
        println!("cargo:rerun-if-changed={path}");
        let ast = syn::parse_file(&fs::read_to_string(&path)?).map_err(io::Error::other)?;
        for route in *routes {
            let method = ast
                .items
                .iter()
                .find_map(|item| {
                    let Item::Impl(item) = item else { return None };
                    item.items.iter().find_map(|item| match item {
                        ImplItem::Fn(method) if method.sig.ident == *route => Some(method),
                        _ => None,
                    })
                })
                .ok_or_else(|| {
                    io::Error::other(format!("Missing command dispatch route {source}:{route}"))
                })?;
            names.visit_block(&method.block);
        }
    }
    fs::write(
        out.join("dispatched_commands.rs"),
        format!("&{:?}", names.0.into_iter().collect::<Vec<_>>()),
    )
}

fn command(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident("name") || path.path.is_ident("command"))
}

#[derive(Default)]
struct Names(BTreeSet<String>);
impl Names {
    fn literal(&mut self, lit: &Lit) {
        if let Lit::Str(name) = lit {
            self.0.insert(name.value());
        }
    }

    fn pattern(&mut self, pat: &Pat) {
        match pat {
            Pat::Lit(lit) => self.literal(&lit.lit),
            Pat::Or(or) => or.cases.iter().for_each(|p| self.pattern(p)),
            Pat::Paren(p) => self.pattern(&p.pat),
            _ => (),
        }
    }
}

impl<'ast> Visit<'ast> for Names {
    fn visit_expr_match(&mut self, expr: &'ast syn::ExprMatch) {
        if command(&expr.expr) {
            expr.arms.iter().for_each(|arm| self.pattern(&arm.pat));
        }
        visit::visit_expr_match(self, expr);
    }

    fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
        if matches!(expr.op, BinOp::Eq(_)) {
            for (name, value) in [(&expr.left, &expr.right), (&expr.right, &expr.left)] {
                if command(name)
                    && let Expr::Lit(lit) = value.as_ref()
                {
                    self.literal(&lit.lit);
                }
            }
        }
        visit::visit_expr_binary(self, expr);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.is_ident("matches")
            && let Ok(Matches(expr, pattern)) = mac.parse_body()
            && command(&expr)
        {
            self.pattern(&pattern);
        }
    }
}

struct Matches(Expr, Pat);
impl Parse for Matches {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let pattern = Pat::parse_multi_with_leading_vert(input)?;
        // Optional guard / trailing comma.
        let _: proc_macro_tokens::Tail = input.parse()?;
        Ok(Self(expr, pattern))
    }
}

mod proc_macro_tokens {
    use syn::{
        Token,
        parse::{Parse, ParseStream},
    };
    pub(super) struct Tail;
    impl Parse for Tail {
        fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
            if input.peek(Token![if]) {
                input.parse::<Token![if]>()?;
                input.parse::<syn::Expr>()?;
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            Ok(Self)
        }
    }
}
